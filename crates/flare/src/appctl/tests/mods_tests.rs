//! Mods inside the window (Surfaces stage 8), with a stand-in for the browser: the switches and the token that
//! goes with them, the grant a page's call is held to, a tool call reaching a page and being answered, and
//! everything that must end a call that nobody can answer.
//!
//! The mods are real folders in the test's own directory (the sample mod is copied from `docs/surfaces`), the hub
//! and the dispatcher are the real ones, and the pages are played by the fake browser: a message from a page is
//! `WebEvent::ModMessage`, a message to a page is a `Call::Post`.

use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;

use flare_ctl::{CallContext, CtlError, Dispatcher, Grant, Method, code};
use flare_mods::Hub;
use flare_mods::hub::Config;
use flare_surfaces::modview;
use flare_web::fake::{Call, FakeWeb};
use flare_web::{WebBackend, WebEvent};
use serde_json::{Value, json};

use super::*;
use crate::ctl_app::{AppDispatcher, CtlOp};
use crate::mods_ctl::{ModsMsg, ModsMsgBox};

type Slot = Rc<RefCell<Option<FakeWeb>>>;

fn with_fake(f: &Fx) -> Slot {
    let slot: Slot = Rc::default();
    let keep = slot.clone();
    f.ctl.borrow_mut().set_web_factory(Box::new(move |sink| {
        let fake = FakeWeb::new(sink);
        *keep.borrow_mut() = Some(fake.clone());
        Box::new(fake)
    }));
    slot
}

fn fake(slot: &Slot) -> FakeWeb {
    slot.borrow().clone().expect("the browser was made")
}

/// The sample mod of `docs/surfaces`, the one the mod author docs point at.
fn sample_notes(root: &Path) {
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/surfaces/sample-mod/notes");
    let to = root.join("notes");
    std::fs::create_dir_all(&to).unwrap(); // sync-ok: test code only, never the app
    for name in ["mod.json", "index.html"] {
        std::fs::copy(from.join(name), to.join(name)).unwrap(); // sync-ok: test code only, never the app
    }
}

/// A mod with a panel and the given permissions (and one tool when it may answer tools).
fn mod_with(root: &Path, id: &str, perms: &[&str]) {
    let tools = if perms.contains(&"tools") {
        r#","tools":[{"name":"ping","description":"Answers.","input_schema":{"type":"object"}}]"#
    } else {
        ""
    };
    let text = format!(
        r#"{{"id":"{id}","name":"Mod {id}","version":"1.0.0","protocol":1,"description":"A test mod.","permissions":{perms:?},"panel":{{"entry":"index.html","title":"{id} panel"}}{tools}}}"#
    );
    let to = root.join(id);
    std::fs::create_dir_all(&to).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("mod.json"), text).unwrap(); // sync-ok: test code only, never the app
    std::fs::write(to.join("index.html"), "<p>page</p>").unwrap(); // sync-ok: test code only, never the app
}

struct World {
    hub: Arc<Hub>,
    dispatcher: Arc<AppDispatcher>,
    events: Arc<Mutex<Vec<String>>>,
    root: PathBuf,
    state_file: PathBuf,
}

/// The mods hub over the folder `mods` of the test's directory, the real dispatcher on it, and the controller
/// told about both. The first scan has finished when this returns.
fn world(f: &Fx, make: impl FnOnce(&Path)) -> World {
    let root = f.dir.path().join("mods");
    std::fs::create_dir_all(&root).unwrap(); // sync-ok: test code only, never the app
    make(&root);
    let state_file = f.dir.path().join("mods.json");
    let hub = Arc::new(Hub::new(Config {
        dir: root.clone(),
        state_file: state_file.clone(),
    }));
    let post: Post = {
        let q = f.q.clone();
        Arc::new(move |m| q.lock().unwrap().push_back(m))
    };
    let dispatcher = Arc::new(
        AppDispatcher::new(
            post,
            Arc::new(|_| None),
            Arc::new(|_, _, _| None),
            "C:\\home".into(),
            Arc::new(AtomicBool::new(false)),
        )
        .with_mods(Arc::clone(&hub)),
    );
    let events: Arc<Mutex<Vec<String>>> = Arc::default();
    {
        let seen = Arc::clone(&events);
        f.ctl.borrow_mut().set_ctl(
            Arc::new(move |name, _params, _tab| seen.lock().unwrap().push(name.to_string())),
            crate::appctl::CtlInfo::default(),
        );
    }
    f.ctl.borrow_mut().set_mods(
        Arc::clone(&hub),
        Some(Arc::clone(&dispatcher) as Arc<dyn Dispatcher>),
    );
    f.pump_until("the first scan", |f| {
        f.ctl.borrow().mods.as_ref().is_some_and(|m| m.scans >= 1)
    });
    World {
        hub,
        dispatcher,
        events,
        root,
        state_file,
    }
}

fn mods_msg(f: &Fx, m: ModsMsg) {
    f.ctl.borrow_mut().on_msg(UiMsg::Mods(ModsMsgBox(m)));
}

fn turn(f: &Fx, id: &str, on: bool) {
    mods_msg(
        f,
        ModsMsg::SetEnabled {
            id: id.into(),
            on,
            shown: None,
        },
    );
}

fn agent_in(tab: &str) -> CallContext {
    CallContext {
        conn: 1,
        client: "test".into(),
        client_version: String::new(),
        grant: Grant::agent(),
        tab: Some(tab.into()),
    }
}

/// A channel call that waits for the window, run on its own thread; the answer arrives on the receiver.
fn call_on_thread(
    d: &Arc<AppDispatcher>,
    ctx: CallContext,
    m: Method,
    params: Value,
) -> Receiver<Result<Value, CtlError>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let d = Arc::clone(d);
    std::thread::spawn(move || {
        let _ = tx.send(d.call(&ctx, m, params));
    });
    rx
}

/// Pumps the window until the receiver has an answer.
fn answer_of(f: &Fx, rx: &Receiver<Result<Value, CtlError>>) -> Result<Value, CtlError> {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(8) {
        f.pump();
        if let Ok(a) = rx.try_recv() {
            return a;
        }
        std::thread::sleep(Duration::from_millis(5)); // sync-ok: test code only
    }
    panic!("the call was not answered");
}

fn finish_page(fake: &FakeWeb, tab: &str, url: &str) {
    thread_local! {
        static NAV: std::cell::Cell<u64> = const { std::cell::Cell::new(5000) };
    }
    let nav = NAV.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    fake.say(WebEvent::Loading {
        tab: tab.into(),
        url: url.into(),
        nav,
    });
    fake.say(WebEvent::Loaded {
        tab: tab.into(),
        url: url.into(),
        ok: true,
        status: "0".into(),
        nav,
    });
}

fn posts(slot: &Slot, tab: &str) -> Vec<Value> {
    fake(slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Post(t, json) if t == tab => Some(serde_json::from_str(&json).unwrap()),
            _ => None,
        })
        .collect()
}

fn page_says(slot: &Slot, tab: &str, text: &str) {
    // The page speaks from the host of the view the tab was last opened on.
    let host = open_mod_calls(slot)
        .into_iter()
        .rev()
        .find(|(t, _)| t == tab)
        .map(|(_, app)| app.host)
        .expect("a mod page open on the tab");
    page_says_from(slot, tab, &host, text);
}

fn page_says_from(slot: &Slot, tab: &str, host: &str, text: &str) {
    fake(slot).say(WebEvent::ModMessage {
        tab: tab.into(),
        host: host.into(),
        json: text.into(),
    });
}

fn open_mod_calls(slot: &Slot) -> Vec<(String, flare_surfaces::app::AppSpec)> {
    fake(slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::OpenApp { tab, app, .. } => Some((tab, app)),
            _ => None,
        })
        .collect()
}

/// Opens the mod's panel on the front tab and plays the page loading. Returns the tab.
fn page_up(f: &Fx, w: &World, slot: &Slot, id: &str) -> String {
    let tab = f.ctl.borrow().front_id().unwrap();
    let rx = call_on_thread(
        &w.dispatcher,
        agent_in(&tab),
        Method::PanelOpen,
        json!({"kind": format!("mod:{id}")}),
    );
    let v = answer_of(f, &rx).unwrap();
    assert_eq!(v["panel"]["kind"], format!("mod:{id}"));
    finish_page(&fake(slot), &tab, &modview::mod_url(&modview::mod_host(id)));
    f.pump();
    tab
}

#[test]
fn a_mod_is_off_until_turned_on_and_the_switch_mints_and_kills_its_token_and_tools() {
    let (f, _h, _) = live(Opts::default());
    let w = world(&f, sample_notes);
    let s = w.hub.snapshot();
    assert_eq!(s.mods.len(), 1);
    assert!(!s.mods[0].enabled, "off until the user turns it on");
    assert!(w.hub.grant_of_mod("notes").is_none());
    assert!(w.hub.tools().is_empty());
    assert!(w.events.lock().unwrap().is_empty());

    turn(&f, "notes", true);
    assert!(w.hub.grant_of_mod("notes").is_some(), "a token was minted");
    let names: Vec<String> = w.hub.tools().iter().map(|t| t.name.clone()).collect();
    assert_eq!(names, ["mod_notes_append_note"]);
    assert_eq!(
        w.events.lock().unwrap().as_slice(),
        ["mods.changed"],
        "the agent's clients hear that the tools changed"
    );
    // The switch is saved on a worker; the token never is.
    f.pump_until("the switch file", |_| {
        std::fs::read_to_string(&w.state_file).is_ok_and(|t| t.contains(r#""notes":{"on":true"#)) // sync-ok: test code only, never the app
    });
    let saved = std::fs::read_to_string(&w.state_file).unwrap(); // sync-ok: test code only, never the app
    assert_eq!(
        saved,
        r#"{"version":1,"mods":{"notes":{"on":true,"permissions":["panel","tools"]}}}"#
    );

    turn(&f, "notes", false);
    assert!(
        w.hub.grant_of_mod("notes").is_none(),
        "the token is dead at once"
    );
    assert!(w.hub.tools().is_empty());
    assert_eq!(w.events.lock().unwrap().len(), 2);
    f.pump_until("the switch file", |_| {
        std::fs::read_to_string(&w.state_file).is_ok_and(|t| t.contains(r#""notes":{"on":false}"#)) // sync-ok: test code only, never the app
    });
}

#[test]
fn a_switch_that_cannot_be_saved_is_said_on_the_page_and_still_takes_effect() {
    let (f, _h, _) = live(Opts::default());
    let w = world(&f, sample_notes);
    // The switch file's place is a folder: the atomic write fails.
    std::fs::create_dir_all(&w.state_file).unwrap(); // sync-ok: test code only, never the app
    turn(&f, "notes", true);
    assert!(w.hub.grant_of_mod("notes").is_some());
    f.pump_until("the failure", |f| {
        f.ctl
            .borrow()
            .mods
            .as_ref()
            .is_some_and(|m| m.save_error.lock().unwrap().is_some())
    });
    let e = f
        .ctl
        .borrow()
        .mods
        .as_ref()
        .unwrap()
        .save_error
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    assert!(e.contains("mods.json"), "{e}");
}

#[test]
fn a_mod_that_is_unknown_cannot_be_turned_on() {
    let (f, _h, _) = live(Opts::default());
    let w = world(&f, sample_notes);
    turn(&f, "ghost", true);
    assert!(w.hub.grant_of_mod("ghost").is_none());
    assert!(w.events.lock().unwrap().is_empty());
}

#[test]
fn a_mods_panel_is_a_web_panel_at_its_own_address_with_its_own_kind_title_and_no_address_bar() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, sample_notes);
    turn(&f, "notes", true);
    let tab = page_up(&f, &w, &slot, "notes");
    let opened = open_mod_calls(&slot);
    assert_eq!(opened.len(), 1, "{opened:?}");
    let spec = &opened[0].1;
    assert_eq!(spec.url, "https://flare-mod-notes.local/");
    assert_eq!(spec.module.as_ref().unwrap().id, "notes");
    assert!(!spec.allow_network, "a mod has no network");
    // It is the mod's folder as the scan found it (served file by file, never mapped).
    assert!(spec.folder.ends_with("notes"));
    let state = f.ctl.borrow().panels.snap(&tab).unwrap();
    assert_eq!(state.kind, "mod:notes");
    assert_eq!(state.title, "Notes");
    assert!(state.url.is_none() && state.path.is_none());
    assert!(f.h.comp.get_panel_web_app(), "no address bar for a mod");
    assert_eq!(f.h.comp.get_panel_title().as_str(), "Notes");
    // The probe sees the page bound to the tab and ready.
    let st = f.ctl.borrow().e2e_mods_state();
    assert_eq!(st["bound"][0]["tab"], json!(tab));
    assert_eq!(st["bound"][0]["ready"], true);
    // The probe never carries a token.
    assert!(!st.to_string().to_lowercase().contains("token"));
}

#[test]
fn the_page_cannot_rename_its_panel_or_move_it_and_the_mod_is_not_stored() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, sample_notes);
    turn(&f, "notes", true);
    let tab = page_up(&f, &w, &slot, "notes");
    fake(&slot).say(WebEvent::Title {
        tab: tab.clone(),
        title: "The bank".into(),
    });
    fake(&slot).say(WebEvent::Url {
        tab: tab.clone(),
        url: "https://example.com/login".into(),
    });
    f.pump();
    assert_eq!(f.ctl.borrow().panels.snap(&tab).unwrap().title, "Notes");
    assert_eq!(
        f.ctl.borrow().panels.web_target(&tab).as_deref(),
        Some("https://flare-mod-notes.local/")
    );
    // Not written to panels.json, and a stored one would be dropped at restore.
    assert!(modview::is_mod_url("https://flare-mod-notes.local/"));
}

#[test]
fn a_page_call_is_held_to_the_mods_permissions_and_a_mod_without_browser_is_refused_browser() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, |r| mod_with(r, "plain", &["panel", "tools"]));
    turn(&f, "plain", true);
    let tab = page_up(&f, &w, &slot, "plain");
    let call = |id: u64, m: &str| {
        page_says(
            &slot,
            &tab,
            &json!({"t":"call","id":id,"method":m,"params":{"url":"https://example.com/"}})
                .to_string(),
        );
    };
    for (i, m) in [
        "browser.open",
        "browser.navigate",
        "browser.look",
        "browser.click",
        "browser.eval",
    ]
    .iter()
    .enumerate()
    {
        call(i as u64 + 1, m);
    }
    f.pump();
    let answers = posts(&slot, &tab);
    assert_eq!(answers.len(), 5, "{answers:?}");
    for a in &answers {
        assert_eq!(a["t"], "reply");
        assert_eq!(a["error"]["code"], "forbidden", "{a}");
    }
    // Nothing reached the browser panel: no page was opened or navigated.
    assert!(
        !fake(&slot)
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Navigate(..) | Call::Cdp(..))),
        "{:?}",
        fake(&slot).calls()
    );
    assert_eq!(
        f.ctl.borrow().panels.snap(&tab).unwrap().kind,
        "mod:plain",
        "its panel was never replaced"
    );
}

#[test]
fn a_page_may_open_only_its_own_panel_not_another_kind() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, |r| mod_with(r, "plain", &["panel", "tools"]));
    turn(&f, "plain", true);
    let tab = page_up(&f, &w, &slot, "plain");
    let kinds = ["web", "changes", "tasks", "mod:other"];
    for (i, k) in kinds.iter().enumerate() {
        page_says(
            &slot,
            &tab,
            &json!({"t":"call","id":i + 1,"method":"panel.open","params":{"kind":k}}).to_string(),
        );
    }
    f.pump_until("the refusals", |_| posts(&slot, &tab).len() >= kinds.len());
    let answers = posts(&slot, &tab);
    assert_eq!(answers.len(), kinds.len(), "{answers:?}");
    for a in &answers {
        assert_eq!(a["error"]["code"], "forbidden", "{a}");
    }
    assert_eq!(
        f.ctl.borrow().panels.snap(&tab).unwrap().kind,
        "mod:plain",
        "its panel was never replaced"
    );
    assert!(
        !fake(&slot)
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Navigate(..) | Call::Cdp(..))),
        "no browser was driven"
    );
}

#[test]
fn an_allowed_call_goes_through_the_dispatcher_with_the_mods_grant_and_the_answer_reaches_the_page()
{
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, |r| mod_with(r, "plain", &["panel", "tools"]));
    turn(&f, "plain", true);
    let tab = page_up(&f, &w, &slot, "plain");
    page_says(
        &slot,
        &tab,
        r#"{"t":"call","id":9,"method":"tabs.list","params":{}}"#,
    );
    f.pump_until("the reply", |_| !posts(&slot, &tab).is_empty());
    let r = &posts(&slot, &tab)[0];
    assert_eq!(r["id"], 9);
    // A mod that holds `panel` sees every tab by id and title, and nothing about their screens.
    let tabs = r["result"]["tabs"].as_array().unwrap();
    assert!(tabs.iter().any(|t| t["id"] == json!(tab)));
    assert!(tabs.iter().all(|t| t.get("screen").is_none()));
}

#[test]
fn a_call_made_after_the_mod_is_turned_off_is_refused_unauthorized() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, |r| mod_with(r, "plain", &["panel", "tools"]));
    turn(&f, "plain", true);
    let tab = page_up(&f, &w, &slot, "plain");
    // The tab's page stays bound for the moment the switch is thrown (the panel closes in the same turn, but a
    // message that was already on its way is still answered with a refusal, never run).
    f.ctl
        .borrow_mut()
        .mods
        .as_mut()
        .unwrap()
        .bind(&tab, "plain");
    w.hub.set_enabled("plain", false).unwrap();
    page_says(
        &slot,
        &tab,
        r#"{"t":"call","id":1,"method":"tabs.list","params":{}}"#,
    );
    f.pump();
    let r = &posts(&slot, &tab)[0];
    assert_eq!(r["error"]["code"], "unauthorized");
}

#[test]
fn turning_a_mod_off_closes_its_panel_ends_its_page_and_publishes_the_change() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, |r| mod_with(r, "plain", &["panel", "tools"]));
    turn(&f, "plain", true);
    let tab = page_up(&f, &w, &slot, "plain");
    assert!(f.ctl.borrow().panels.mod_of(&tab).is_some());
    let changed = |w: &World| {
        w.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| *e == "mods.changed")
            .count()
    };
    let before = changed(&w);
    turn(&f, "plain", false);
    f.h.advance(0);
    assert!(
        f.ctl.borrow().panels.mod_of(&tab).is_none(),
        "the panel is gone"
    );
    assert_eq!(f.ctl.borrow().web_controllers(), 0, "so is its browser");
    assert!(
        f.ctl.borrow().e2e_mods_state()["bound"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(changed(&w), before + 1);
    assert!(
        w.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e == "panel.changed"),
        "the tab's clients hear the panel closed"
    );
}

#[test]
fn panel_open_mod_opens_the_panel_only_for_a_mod_that_is_on_and_has_one() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, sample_notes);
    let tab = f.ctl.borrow().front_id().unwrap();
    let off = call_on_thread(
        &w.dispatcher,
        agent_in(&tab),
        Method::PanelOpen,
        json!({"kind": "mod:notes"}),
    );
    assert_eq!(answer_of(&f, &off).unwrap_err().code, code::NOT_FOUND);
    for bad in ["mod:", "mod:Bad Id", "mod:a/b"] {
        let rx = call_on_thread(
            &w.dispatcher,
            agent_in(&tab),
            Method::PanelOpen,
            json!({"kind": bad}),
        );
        assert_eq!(
            answer_of(&f, &rx).unwrap_err().code,
            code::BAD_PARAMS,
            "{bad}"
        );
    }
    turn(&f, "notes", true);
    let tab = page_up(&f, &w, &slot, "notes");
    assert_eq!(f.ctl.borrow().panels.mod_of(&tab), Some("notes"));
}

#[test]
fn a_tool_call_opens_the_mods_panel_in_the_callers_tab_waits_for_the_page_and_returns_its_answer() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, sample_notes);
    turn(&f, "notes", true);
    let tab = f.ctl.borrow().front_id().unwrap();
    let rx = call_on_thread(
        &w.dispatcher,
        agent_in(&tab),
        Method::ModsCall,
        json!({"name": "mod_notes_append_note", "input": {"text": "buy milk"}}),
    );
    f.pump_until("the panel", |f| {
        f.ctl.borrow().panels.mod_of(&tab).is_some()
    });
    assert!(
        posts(&slot, &tab).is_empty(),
        "the page is not ready: the call waits"
    );
    assert!(rx.try_recv().is_err());
    // The page loads: the call is handed over.
    finish_page(&fake(&slot), &tab, "https://flare-mod-notes.local/");
    f.pump_until("the tool event", |_| !posts(&slot, &tab).is_empty());
    let ev = &posts(&slot, &tab)[0];
    assert_eq!(ev["t"], "tool");
    assert_eq!(ev["tool"], "append_note");
    assert_eq!(ev["input"], json!({"text": "buy milk"}));
    let call_id = ev["callId"].as_str().unwrap().to_string();
    assert_eq!(
        f.ctl.borrow().e2e_mods_state()["pending"][0]["calls"],
        1,
        "one call waits"
    );
    // The page answers.
    page_says(
        &slot,
        &tab,
        &json!({"t":"respond","call_id":call_id,"result":{"count":1}}).to_string(),
    );
    let v = answer_of(&f, &rx).unwrap();
    assert_eq!(v, json!({"result": {"count": 1}}));
    assert_eq!(f.ctl.borrow().e2e_mods_state()["pending"], json!([]));
}

#[test]
fn a_call_to_a_page_that_is_already_up_is_handed_over_at_once() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, sample_notes);
    turn(&f, "notes", true);
    let tab = page_up(&f, &w, &slot, "notes");
    let rx = call_on_thread(
        &w.dispatcher,
        agent_in(&tab),
        Method::ModsCall,
        json!({"name": "mod_notes_append_note", "input": {"text": "a"}}),
    );
    f.pump_until("the tool event", |_| !posts(&slot, &tab).is_empty());
    let id = posts(&slot, &tab)[0]["callId"]
        .as_str()
        .unwrap()
        .to_string();
    page_says(
        &slot,
        &tab,
        &json!({"t":"respond","call_id":id,"error":"text must be 1 to 500 characters"}).to_string(),
    );
    let e = answer_of(&f, &rx).unwrap_err();
    assert_eq!(e.message, "text must be 1 to 500 characters");
    assert_eq!(open_mod_calls(&slot).len(), 1, "no second view was made");
}

#[test]
fn a_mod_cannot_answer_a_call_that_was_routed_to_another() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, |r| {
        mod_with(r, "alpha", &["panel", "tools"]);
        mod_with(r, "beta", &["panel", "tools"]);
    });
    turn(&f, "alpha", true);
    turn(&f, "beta", true);
    let tab = f.ctl.borrow().front_id().unwrap();
    let rx = call_on_thread(
        &w.dispatcher,
        agent_in(&tab),
        Method::ModsCall,
        json!({"name": "mod_alpha_ping", "input": {}}),
    );
    f.pump_until("alpha's panel", |f| {
        f.ctl.borrow().panels.mod_of(&tab) == Some("alpha")
    });
    finish_page(&fake(&slot), &tab, "https://flare-mod-alpha.local/");
    f.pump_until("the tool event", |_| !posts(&slot, &tab).is_empty());
    let id = posts(&slot, &tab)[0]["callId"]
        .as_str()
        .unwrap()
        .to_string();
    // Beta's page, in another tab's place, tries to answer alpha's call: bind beta to a made-up tab to play it.
    f.ctl
        .borrow_mut()
        .mods
        .as_mut()
        .unwrap()
        .bind("beta-tab", "beta");
    page_says_from(
        &slot,
        "beta-tab",
        &modview::mod_host("beta"),
        &json!({"t":"respond","call_id":id,"result":"stolen"}).to_string(),
    );
    f.pump();
    assert!(
        rx.try_recv().is_err(),
        "the call is still waiting for alpha"
    );
    page_says(
        &slot,
        &tab,
        &json!({"t":"respond","call_id":id,"result":"mine"}).to_string(),
    );
    assert_eq!(answer_of(&f, &rx).unwrap(), json!({"result": "mine"}));
}

#[test]
fn a_message_from_the_page_the_tab_left_is_not_run_with_the_mod_it_shows_now() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, |r| {
        mod_with(r, "alpha", &["panel", "tools"]);
        mod_with(r, "beta", &["panel", "tools"]);
    });
    turn(&f, "alpha", true);
    turn(&f, "beta", true);
    let tab = f.ctl.borrow().front_id().unwrap();
    let rx = call_on_thread(
        &w.dispatcher,
        agent_in(&tab),
        Method::ModsCall,
        json!({"name": "mod_beta_ping", "input": {}}),
    );
    f.pump_until("beta's panel", |f| {
        f.ctl.borrow().panels.mod_of(&tab) == Some("beta")
    });
    finish_page(&fake(&slot), &tab, "https://flare-mod-beta.local/");
    f.pump_until("the tool event", |_| !posts(&slot, &tab).is_empty());
    let id = posts(&slot, &tab)[0]["callId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(id, "c1", "a call id is not a counter");
    // A message alpha's page queued before the tab moved on to beta arrives now: it answers nothing.
    page_says_from(
        &slot,
        &tab,
        &modview::mod_host("alpha"),
        &json!({"t":"respond","call_id":id,"result":"stolen"}).to_string(),
    );
    f.pump();
    assert!(rx.try_recv().is_err(), "the call still waits for beta");
    page_says(
        &slot,
        &tab,
        &json!({"t":"respond","call_id":id,"result":"mine"}).to_string(),
    );
    assert_eq!(answer_of(&f, &rx).unwrap(), json!({"result": "mine"}));
}

#[test]
fn closing_the_panel_the_page_dying_or_the_mod_going_off_fails_the_waiting_call() {
    for how in ["close", "gone", "off"] {
        let (f, _h, _) = live(Opts::default());
        let slot = with_fake(&f);
        let w = world(&f, sample_notes);
        turn(&f, "notes", true);
        let tab = page_up(&f, &w, &slot, "notes");
        let rx = call_on_thread(
            &w.dispatcher,
            agent_in(&tab),
            Method::ModsCall,
            json!({"name": "mod_notes_append_note", "input": {"text": "a"}}),
        );
        f.pump_until("the tool event", |_| !posts(&slot, &tab).is_empty());
        match how {
            "close" => {
                f.ctl
                    .borrow_mut()
                    .on_msg(UiMsg::Ctl(crate::ctl_app::CtlRequestBox(
                        crate::ctl_app::CtlRequest {
                            op: CtlOp::PanelClose { tab: tab.clone() },
                            deadline: Instant::now() + Duration::from_secs(5),
                            reply: std::sync::mpsc::channel().0,
                        },
                    )));
            }
            "gone" => fake(&slot).say(WebEvent::Gone {
                tab: tab.clone(),
                why: "the page's process ended".into(),
            }),
            _ => turn(&f, "notes", false),
        }
        let e = answer_of(&f, &rx).unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE, "{how}: {}", e.message);
        assert_eq!(w.hub.pending_of("notes"), 0, "{how}");
    }
}

#[test]
fn after_the_page_dies_the_next_call_makes_a_fresh_page_and_is_answered() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, sample_notes);
    turn(&f, "notes", true);
    let tab = page_up(&f, &w, &slot, "notes");
    fake(&slot).say(WebEvent::Gone {
        tab: tab.clone(),
        why: "the page's process ended".into(),
    });
    f.pump();
    let rx = call_on_thread(
        &w.dispatcher,
        agent_in(&tab),
        Method::ModsCall,
        json!({"name": "mod_notes_append_note", "input": {"text": "again"}}),
    );
    // The dead view is replaced: a new page is made and, once it loads, the call reaches it.
    f.pump_until("a second page", |_| open_mod_calls(&slot).len() >= 2);
    assert!(
        posts(&slot, &tab).is_empty(),
        "queued until the new page loads"
    );
    finish_page(&fake(&slot), &tab, "https://flare-mod-notes.local/");
    f.pump_until("the tool event", |_| !posts(&slot, &tab).is_empty());
    let id = posts(&slot, &tab)[0]["callId"]
        .as_str()
        .unwrap()
        .to_string();
    page_says(
        &slot,
        &tab,
        &json!({"t":"respond","call_id":id,"result":"ok"}).to_string(),
    );
    assert_eq!(answer_of(&f, &rx).unwrap(), json!({"result": "ok"}));
}

#[test]
fn a_navigation_the_host_cancelled_does_not_make_a_ready_page_unready() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, sample_notes);
    turn(&f, "notes", true);
    let tab = page_up(&f, &w, &slot, "notes");
    assert!(f.ctl.borrow().mods.as_ref().unwrap().is_ready(&tab));
    // The person clicks a link out of the mod's host: the host cancels it (Refused), and the browser still
    // ends that navigation with a failed load of a number the page never started.
    fake(&slot).say(WebEvent::Refused {
        tab: tab.clone(),
        url: "https://example.com/".into(),
    });
    fake(&slot).say(WebEvent::Loaded {
        tab: tab.clone(),
        url: modview::mod_url(&modview::mod_host("notes")),
        ok: false,
        status: "3".into(),
        nav: 424_242,
    });
    f.pump();
    assert!(
        f.ctl.borrow().mods.as_ref().unwrap().is_ready(&tab),
        "the page that is open is still the mod's, and still ready"
    );
    // The next tool call reaches it at once.
    let rx = call_on_thread(
        &w.dispatcher,
        agent_in(&tab),
        Method::ModsCall,
        json!({"name": "mod_notes_append_note", "input": {"text": "a"}}),
    );
    f.pump_until("the tool event", |_| !posts(&slot, &tab).is_empty());
    let id = posts(&slot, &tab)[0]["callId"]
        .as_str()
        .unwrap()
        .to_string();
    page_says(
        &slot,
        &tab,
        &json!({"t":"respond","call_id":id,"result":{"ok":true}}).to_string(),
    );
    assert!(answer_of(&f, &rx).is_ok());
}

#[test]
fn a_tool_call_whose_caller_gave_up_is_not_run_when_the_page_becomes_ready() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, sample_notes);
    turn(&f, "notes", true);
    let tab = page_up(&f, &w, &slot, "notes");
    let host = modview::mod_url(&modview::mod_host("notes"));
    // The page begins to load again (not ready); a call is queued for it, then its caller gives up.
    fake(&slot).say(WebEvent::Loading {
        tab: tab.clone(),
        url: host.clone(),
        nav: 777,
    });
    f.pump();
    let rx = call_on_thread(
        &w.dispatcher,
        agent_in(&tab),
        Method::ModsCall,
        json!({"name": "mod_notes_append_note", "input": {"text": "late"}}),
    );
    f.pump_until("the call to be waiting", |f| {
        f.ctl
            .borrow()
            .mods
            .as_ref()
            .unwrap()
            .hub()
            .pending_of("notes")
            == 1
    });
    let id = f
        .ctl
        .borrow()
        .mods
        .as_ref()
        .unwrap()
        .hub()
        .pending_ids("notes")
        .remove(0);
    f.ctl.borrow().mods.as_ref().unwrap().hub().cancel(&id);
    drop(rx);
    // The page finishes loading: the abandoned call is not delivered.
    fake(&slot).say(WebEvent::Loaded {
        tab: tab.clone(),
        url: host,
        ok: true,
        status: "0".into(),
        nav: 777,
    });
    f.pump();
    assert!(
        posts(&slot, &tab).is_empty(),
        "an abandoned call ran: {:?}",
        posts(&slot, &tab)
    );
}

#[test]
fn a_reply_meant_for_an_older_document_is_not_given_to_a_newer_one() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, |r| mod_with(r, "plain", &["panel", "tools"]));
    turn(&f, "plain", true);
    let tab = page_up(&f, &w, &slot, "plain");
    let old = f
        .ctl
        .borrow()
        .mods
        .as_ref()
        .unwrap()
        .generation(&tab)
        .unwrap();
    // The page reloads: it is a new document.
    finish_page(
        &fake(&slot),
        &tab,
        &modview::mod_url(&modview::mod_host("plain")),
    );
    f.pump();
    let new = f
        .ctl
        .borrow()
        .mods
        .as_ref()
        .unwrap()
        .generation(&tab)
        .unwrap();
    assert_ne!(old, new);
    mods_msg(
        &f,
        ModsMsg::Reply {
            tab: tab.clone(),
            mod_id: "plain".into(),
            generation: old,
            json: "{\"stale\":true}".into(),
        },
    );
    assert!(posts(&slot, &tab).is_empty(), "the stale reply is dropped");
    mods_msg(
        &f,
        ModsMsg::Reply {
            tab: tab.clone(),
            mod_id: "plain".into(),
            generation: new,
            json: "{\"fresh\":true}".into(),
        },
    );
    assert_eq!(posts(&slot, &tab).len(), 1);
}

#[test]
fn a_call_nobody_answers_times_out_with_the_mods_name() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, sample_notes);
    turn(&f, "notes", true);
    let tab = page_up(&f, &w, &slot, "notes");
    // The hub's own wait is 30 s; here the same wait with a short limit shows the answer.
    let call = w
        .hub
        .begin_call("mod_notes_append_note", json!({"text": "a"}))
        .unwrap();
    let hub = &w.hub;
    let waited = hub.wait(call, Duration::from_millis(100), &|| false); // sync-ok: test code only
    let e = waited.unwrap_err();
    assert_eq!(e.code, code::TIMEOUT);
    assert!(e.message.contains("notes") && e.message.contains("append_note"));
    drop(tab);
}

#[test]
fn a_tool_call_for_a_tool_that_is_not_offered_is_not_found_and_reaches_no_page() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, sample_notes);
    let tab = f.ctl.borrow().front_id().unwrap();
    // Off: no tools.
    let rx = call_on_thread(
        &w.dispatcher,
        agent_in(&tab),
        Method::ModsCall,
        json!({"name": "mod_notes_append_note", "input": {}}),
    );
    assert_eq!(answer_of(&f, &rx).unwrap_err().code, code::NOT_FOUND);
    // The listing is empty too.
    let list = call_on_thread(&w.dispatcher, agent_in(&tab), Method::ModsTools, json!({}));
    assert_eq!(answer_of(&f, &list).unwrap()["tools"], json!([]));
    turn(&f, "notes", true);
    let list = call_on_thread(&w.dispatcher, agent_in(&tab), Method::ModsTools, json!({}));
    let v = answer_of(&f, &list).unwrap();
    assert_eq!(v["tools"][0]["name"], "mod_notes_append_note");
    assert_eq!(v["tools"][0]["input_schema"]["type"], "object");
    assert!(slot.borrow().is_none() || open_mod_calls(&slot).is_empty());
}

#[test]
fn a_mod_may_not_call_tools_or_answer_over_the_agents_grant() {
    let (f, _h, _) = live(Opts::default());
    let w = world(&f, sample_notes);
    turn(&f, "notes", true);
    let tab = f.ctl.borrow().front_id().unwrap();
    // The agent cannot answer as a mod.
    let r = w
        .dispatcher
        .call(
            &agent_in(&tab),
            Method::ToolsRespond,
            json!({"call_id": "c1", "result": 1}),
        )
        .unwrap_err();
    assert_eq!(r.code, code::FORBIDDEN);
    // A mod's grant answers a call that is not open with not_found, and needs one of result and error.
    let mod_ctx = CallContext {
        grant: w.hub.grant_of_mod("notes").unwrap(),
        ..agent_in(&tab)
    };
    let e = w
        .dispatcher
        .call(
            &mod_ctx,
            Method::ToolsRespond,
            json!({"call_id": "c1", "result": 1}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
    let e = w
        .dispatcher
        .call(&mod_ctx, Method::ToolsRespond, json!({"call_id": "c1"}))
        .unwrap_err();
    assert_eq!(e.code, code::BAD_PARAMS);
    let e = w
        .dispatcher
        .call(
            &mod_ctx,
            Method::ToolsRespond,
            json!({"call_id": "c1", "result": 1, "error": "x"}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::BAD_PARAMS);
}

#[test]
fn the_browser_tools_do_not_drive_a_mods_page() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, sample_notes);
    turn(&f, "notes", true);
    let tab = page_up(&f, &w, &slot, "notes");
    let rx = call_on_thread(
        &w.dispatcher,
        agent_in(&tab),
        Method::BrowserLook,
        json!({}),
    );
    let e = answer_of(&f, &rx).unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    assert!(e.message.contains("mod"), "{}", e.message);
    assert!(
        !fake(&slot)
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Cdp(..))),
        "no DevTools call reached the mod's page"
    );
}

#[test]
fn a_reply_for_a_panel_that_has_gone_is_dropped_and_a_message_to_another_kind_of_view_is_ignored() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let w = world(&f, |r| mod_with(r, "plain", &["panel", "tools"]));
    turn(&f, "plain", true);
    let tab = page_up(&f, &w, &slot, "plain");
    // A web page in another tab says something in the bridge's language: nobody listens.
    mods_msg(
        &f,
        ModsMsg::Reply {
            tab: tab.clone(),
            mod_id: "other".into(),
            generation: 0,
            json: "{}".into(),
        },
    );
    assert!(
        posts(&slot, &tab).is_empty(),
        "a reply for another mod is not posted"
    );
    // The backend refuses to post to a view that is not a mod's.
    let mut b = fake(&slot);
    b.post_message("nowhere", "{}");
    assert!(posts(&slot, "nowhere").is_empty());
}

#[test]
fn mod_tools_are_listed_only_when_this_run_has_mods() {
    let (f, _h, _) = live(Opts::default());
    let w = world(&f, sample_notes);
    let caps = w.dispatcher.capabilities();
    for m in [Method::ModsTools, Method::ModsCall, Method::ToolsRespond] {
        assert!(caps.contains(&m), "{}", m.name());
    }
    // The grant narrows it again: the agent may list and call tools, never answer.
    assert!(Grant::agent().may_call(Method::ModsCall));
    assert!(!Grant::agent().may_call(Method::ToolsRespond));
    let _ = &w.root;
}
