//! Import remote sessions PR 3 through the real window controller (rules 1291 to 1295): a fake watch link hands the
//! window a linked machine's `remote.status`, and the fake remote side of the spawner shows which rows attached and how.
//! No ssh, no other machine, no window on the screen.

use std::collections::HashSet;

use flare_term::remote_pty::{Leaving, LinkState, RemoteSpec};
use serde_json::{Value, json};

use super::*;
use crate::remote_import::{Heard, Watch, WatchAnswer, WatchPort, WatchPost};

const SESSION: &str = "3f2a9c1e-77aa-4c0d-9b1e-0a1b2c3d4e5f";

/// The watch links the window asked for: which machines are watched now, and every close sent over one.
#[derive(Default)]
struct Watches {
    live: Mutex<HashSet<String>>,
    started: Mutex<Vec<String>>,
    closes: Mutex<Vec<(String, String)>>,
}

struct FakeWatch {
    machine: String,
    w: Arc<Watches>,
}

impl Watch for FakeWatch {
    fn close(&self, host_tab: &str) {
        self.w
            .closes
            .lock()
            .unwrap()
            .push((self.machine.clone(), host_tab.to_string()));
    }
}

impl Drop for FakeWatch {
    fn drop(&mut self) {
        self.w.live.lock().unwrap().remove(&self.machine);
    }
}

struct FakePort(Arc<Watches>);

impl WatchPort for FakePort {
    fn watch(&self, machine: &str, _post: WatchPost) -> Box<dyn Watch> {
        self.0.live.lock().unwrap().insert(machine.to_string());
        self.0.started.lock().unwrap().push(machine.to_string());
        Box::new(FakeWatch {
            machine: machine.to_string(),
            w: Arc::clone(&self.0),
        })
    }
}

impl Watches {
    fn live(&self) -> Vec<String> {
        let mut v: Vec<String> = self.live.lock().unwrap().iter().cloned().collect();
        v.sort();
        v
    }

    fn closes(&self) -> Vec<(String, String)> {
        self.closes.lock().unwrap().clone()
    }
}

/// The window imports from the Mac Mini: its watch port is the fake, and the machines name the Mac.
fn importing(w: &Wx) -> Arc<Watches> {
    let watches = Arc::new(Watches::default());
    w.ctl
        .borrow_mut()
        .set_watch_port(Arc::new(FakePort(Arc::clone(&watches))));
    w.ctl.borrow_mut().on_msg(UiMsg::Machines(vec![mac_mini()]));
    w.pump();
    watches
}

/// One host tab as the Mac's `remote.status` lists it: a Claude tab its own window sizes, or a plain shell.
fn entry(tab: &str, state: &str, claude: bool) -> Value {
    json!({
        "tab": tab,
        "state": state,
        "kind": if claude { json!("claude") } else { Value::Null },
        "session": if claude { json!(SESSION) } else { Value::Null },
        "folder": format!("/Users/max/{tab}"),
        "title": tab,
        "sized": "host",
        "origin": "here",
    })
}

/// The Mac's watch link heard this listing.
fn heard(w: &Wx, tabs: Vec<Value>) {
    w.ctl.borrow_mut().on_msg(UiMsg::Import(WatchAnswer {
        machine: "mac".into(),
        heard: Heard::Listed(json!({ "tabs": tabs })),
    }));
    w.pump();
}

fn ids(w: &Wx) -> Vec<String> {
    w.ctl
        .borrow()
        .model()
        .state
        .tabs
        .iter()
        .map(|t| t.id.clone())
        .collect()
}

fn front(w: &Wx) -> String {
    w.ctl
        .borrow()
        .model()
        .state
        .active_id
        .clone()
        .expect("a front tab")
}

/// The window's row of the Mac's host tab `host_tab`, if one is shown.
fn row_of(w: &Wx, host_tab: &str) -> Option<String> {
    let c = w.ctl.borrow();
    let m = c.model();
    m.state
        .tabs
        .iter()
        .find(|t| m.remote_live(&t.id).and_then(|l| l.host_tab()).as_deref() == Some(host_tab))
        .map(|t| t.id.clone())
}

/// The remote specs spawned so far: one per attach, in order.
fn attached(w: &Wx) -> Vec<RemoteSpec> {
    w.pump();
    w.remote
        .made()
        .into_iter()
        .filter_map(|(s, _)| s.remote)
        .collect()
}

fn pick(w: &Wx, id: &str) {
    let mut c = w.ctl.borrow_mut();
    c.model_mut().pick(id);
    c.refresh();
}

/// Rules 1290 and 1292: a listing adds a row per host tab at the end of the strip, behind the front tab; nothing links
/// until a row is shown. A click attaches that row only: its own host tab, no `remote.open`, no resume, `ring: "both"`,
/// and a Claude tab the host sizes starts in the host-sizes state. Its session stays, and the host's folder does not
/// join the recent folders.
#[test]
fn an_import_adds_rows_behind_the_front_tab_and_a_click_attaches() {
    let w = Wx::new(1.0);
    let watches = importing(&w);
    assert_eq!(watches.live(), ["mac"], "the setting is on by default");
    let first = front(&w);
    heard(
        &w,
        vec![entry("t1-1", "idle", true), entry("t1-2", "idle", false)],
    );
    let all = ids(&w);
    assert_eq!(all.len(), 3);
    assert_eq!(front(&w), first, "an import never changes the front tab");
    let (a, b) = (row_of(&w, "t1-1").unwrap(), row_of(&w, "t1-2").unwrap());
    assert_eq!(
        all[1..],
        [a.clone(), b.clone()],
        "at the end, in the host's order"
    );
    assert!(w.ctl.borrow().model().is_unattached(&a));
    assert_eq!(
        w.ctl.borrow().model().state.tabs[1].cwd,
        "/Users/max/t1-1",
        "the host's folder"
    );
    assert!(attached(&w).is_empty(), "no link until a row is shown");
    // The same listing again adds nothing.
    heard(
        &w,
        vec![entry("t1-1", "idle", true), entry("t1-2", "idle", false)],
    );
    assert_eq!(ids(&w).len(), 3, "one row per host tab");
    pick(&w, &a);
    w.pump_until("the attach", |w| !w.remote.made().is_empty());
    let specs = attached(&w);
    assert_eq!(specs.len(), 1, "only the row shown attaches");
    let s = &specs[0];
    assert_eq!(s.machine, "mac");
    assert_eq!(
        s.live.host_tab().as_deref(),
        Some("t1-1"),
        "its own host tab: no open"
    );
    assert_eq!(s.resume, None, "never a resume");
    assert!(
        s.host_sized_start,
        "the host sizes it: no size until a click or key"
    );
    assert!(s.ring_both, "its question rings on both machines");
    assert!(!w.ctl.borrow().model().is_unattached(&a));
    assert_eq!(
        w.ctl.borrow_mut().hub_mut().claude_session(&a),
        Some(SESSION),
        "the attach keeps the session the poll gave"
    );
    assert!(
        flare_settings::recent_roots::remote_roots(&w.prefs).is_empty(),
        "the host's folder is not a recent folder here"
    );
    // A plain shell sends its size on attach as a followed tab does.
    pick(&w, &b);
    w.pump_until("the second attach", |w| w.remote.made().len() == 2);
    assert!(!attached(&w)[1].host_sized_start);
}

/// Rule 1292: a Ctrl+Tab walk, in either switch mode, attaches only the row it lands on when Control is let go.
#[test]
fn ctrl_tab_in_order_and_recent_modes_attaches_only_the_row_it_lands_on() {
    for mode in ["order", "recent"] {
        let w = Wx::new(1.0);
        flare_settings::tab_switch::set_tab_switch_word(&w.prefs, mode);
        importing(&w);
        heard(
            &w,
            vec![
                entry("t1-1", "idle", true),
                entry("t1-2", "idle", true),
                entry("t1-3", "idle", true),
            ],
        );
        let tab = || ctrl("Tab", "Tab");
        for _ in 0..2 {
            let _ = w.ctl.borrow_mut().on_key(&tab());
            w.pump();
        }
        let landed = front(&w);
        assert!(
            w.ctl.borrow().model().is_unattached(&landed),
            "{mode}: nothing attaches while Control is held"
        );
        assert!(attached(&w).is_empty(), "{mode}");
        w.ctl
            .borrow_mut()
            .on_key_up(&KeyEvent::new("Control", "ControlLeft").released());
        w.pump_until("the attach", |w| !w.remote.made().is_empty());
        let specs = attached(&w);
        assert_eq!(
            specs.len(),
            1,
            "{mode}: the rows passed over stay unattached"
        );
        let host_tab = specs[0].live.host_tab().unwrap();
        assert_eq!(row_of(&w, &host_tab), Some(landed), "{mode}");
    }
}

/// Rule 1292: a close that fronts an imported row (its right neighbour) attaches it.
#[test]
fn a_close_that_fronts_an_imported_row_attaches_it() {
    let w = Wx::new(1.0);
    importing(&w);
    let first = front(&w);
    heard(&w, vec![entry("t1-1", "idle", false)]);
    let row = row_of(&w, "t1-1").unwrap();
    w.ctl.borrow_mut().request_close(&first);
    w.pump_until("the attach", |w| !w.remote.made().is_empty());
    assert_eq!(front(&w), row);
    assert_eq!(attached(&w)[0].live.host_tab().as_deref(), Some("t1-1"));
}

/// Rule 1290: an open on that machine that still waits for its host tab skips the round; one whose link ended without a
/// host tab does not block the imports for ever.
#[test]
fn an_open_that_ended_without_a_host_tab_does_not_block_imports() {
    let w = Wx::new(1.0);
    importing(&w);
    let opened = w
        .ctl
        .borrow_mut()
        .open_remote("mac", "/Users/max/proj", None, false);
    heard(&w, vec![entry("t1-1", "idle", false)]);
    assert_eq!(
        row_of(&w, "t1-1"),
        None,
        "an open in flight skips the round"
    );
    w.ctl
        .borrow()
        .model()
        .remote_live(&opened)
        .unwrap()
        .set_link(LinkState::Ended);
    heard(&w, vec![entry("t1-1", "idle", false)]);
    assert!(row_of(&w, "t1-1").is_some());
}

/// Rule 1293: Close on an unattached row sends `remote.close` over the machine's watch link (no session link is
/// made); the row goes at once, and while the host still lists the tab (it asks first, rule 48) it does not come back.
#[test]
fn close_on_an_unattached_imported_row_sends_remote_close_and_does_not_come_back_while_the_host_asks()
 {
    let w = Wx::new(1.0);
    let watches = importing(&w);
    heard(
        &w,
        vec![entry("t1-1", "idle", false), entry("t1-2", "idle", false)],
    );
    let row = row_of(&w, "t1-2").unwrap();
    w.ctl.borrow_mut().request_close(&row);
    w.pump();
    assert_eq!(watches.closes(), [("mac".to_string(), "t1-2".to_string())]);
    assert_eq!(row_of(&w, "t1-2"), None, "the row goes at once");
    assert!(attached(&w).is_empty(), "no session link for a close");
    heard(
        &w,
        vec![entry("t1-1", "idle", false), entry("t1-2", "idle", false)],
    );
    assert_eq!(
        row_of(&w, "t1-2"),
        None,
        "the host still asks: it stays out"
    );
    // The host dropped it: a later tab of the same id would come in again.
    heard(&w, vec![entry("t1-1", "idle", false)]);
    heard(
        &w,
        vec![entry("t1-1", "idle", false), entry("t1-2", "idle", false)],
    );
    assert!(row_of(&w, "t1-2").is_some());
}

/// Rule 1293: closing a tab opened from here, which its host keeps listing as its own while it asks, does not bring it
/// back as an import either.
#[test]
fn closing_a_followed_remote_tab_keeps_it_out_while_the_host_asks() {
    let w = Wx::new(1.0);
    let watches = importing(&w);
    let opened = w
        .ctl
        .borrow_mut()
        .open_remote("mac", "/Users/max/proj", None, true);
    w.ctl
        .borrow()
        .model()
        .remote_live(&opened)
        .unwrap()
        .set_host_tab("t1-5");
    w.ctl.borrow_mut().close_now(&opened);
    w.pump();
    assert!(watches.closes().is_empty(), "its own link closes it");
    heard(&w, vec![entry("t1-5", "working", true)]);
    assert_eq!(row_of(&w, "t1-5"), None);
}

/// Rule 1293: Hide here only remembers the host tab, for an imported row and for a tab opened from here; the next
/// polls do not bring either back, and the list is kept in the settings.
#[test]
fn hide_here_only_remembers_the_host_tab_and_the_next_poll_does_not_bring_it_back() {
    let w = Wx::new(1.0);
    let watches = importing(&w);
    heard(&w, vec![entry("t1-1", "idle", true)]);
    let row = row_of(&w, "t1-1").unwrap();
    w.ctl.borrow_mut().hide_remote(&row);
    let opened = w
        .ctl
        .borrow_mut()
        .open_remote("mac", "/Users/max/proj", None, true);
    w.ctl
        .borrow()
        .model()
        .remote_live(&opened)
        .unwrap()
        .set_host_tab("t1-5");
    w.ctl.borrow_mut().hide_remote(&opened);
    assert_eq!(w.ctl.borrow().hidden_of("mac"), ["t1-1", "t1-5"]);
    assert_eq!(
        flare_settings::machine_prefs::import_hidden(&w.prefs)["mac"],
        ["t1-1", "t1-5"]
    );
    for _ in 0..2 {
        heard(
            &w,
            vec![entry("t1-1", "idle", true), entry("t1-5", "idle", true)],
        );
        assert_eq!(row_of(&w, "t1-1"), None);
        assert_eq!(row_of(&w, "t1-5"), None);
    }
    assert!(watches.closes().is_empty(), "a hide never closes");
    // A listing that no longer names a hidden tab prunes it.
    heard(&w, vec![entry("t1-5", "idle", true)]);
    assert_eq!(w.ctl.borrow().hidden_of("mac"), ["t1-5"]);
}

/// Rule 1294: the setting off removes every imported row: an attached one by a detach, never `remote.close`; the front
/// goes to the tab used before; tabs opened from here stay, and the watch stops. On again watches again.
#[test]
fn turning_import_off_detaches_imported_rows_and_keeps_the_rest() {
    let w = Wx::new(1.0);
    let watches = importing(&w);
    let first = front(&w);
    let opened = w
        .ctl
        .borrow_mut()
        .open_remote("mac", "/Users/max/proj", None, false);
    w.ctl
        .borrow()
        .model()
        .remote_live(&opened)
        .unwrap()
        .set_host_tab("t1-9");
    heard(
        &w,
        vec![entry("t1-1", "idle", true), entry("t1-2", "idle", true)],
    );
    let row = row_of(&w, "t1-1").unwrap();
    pick(&w, &row);
    w.pump_until("the attach", |w| w.remote.made().len() == 2);
    // The links start on workers: the row's is found by its host tab, not by its place in the list.
    let io = w
        .remote
        .made()
        .into_iter()
        .find(|(s, _)| s.remote.as_ref().and_then(|r| r.live.host_tab()).as_deref() == Some("t1-1"))
        .map(|(_, io)| io)
        .expect("the row's link");
    flare_settings::machine_prefs::set_import_remote_on(&w.prefs, false);
    w.pump_until("the rows gone", |w| ids(w).len() == 2);
    assert_eq!(
        ids(&w),
        [first.clone(), opened],
        "a tab opened from here stays"
    );
    assert_eq!(front(&w), first, "the tab used before comes to the front");
    let t = Instant::now();
    let mut leaving = None;
    while leaving.is_none() && t.elapsed() < Duration::from_secs(5) {
        leaving = io.take_outgoing(Duration::from_millis(20)).leaving;
    }
    assert_eq!(leaving, Some(Leaving::Detach), "detached, never closed");
    assert!(watches.closes().is_empty());
    assert!(
        watches.live().is_empty(),
        "no watch while the setting is off"
    );
    assert!(w.ctl.borrow().hidden_of("mac").is_empty(), "nothing hidden");
    flare_settings::machine_prefs::set_import_remote_on(&w.prefs, true);
    w.pump_until("the watch again", |_| watches.live() == ["mac"]);
}

/// Rule 1291: a machine removed on Settings > Machines stops its watch; its rows and its hidden list go.
#[test]
fn a_machine_removed_from_settings_stops_its_watcher_and_drops_its_rows_and_hidden_list() {
    let w = Wx::new(1.0);
    let watches = importing(&w);
    heard(
        &w,
        vec![entry("t1-1", "idle", true), entry("t1-2", "idle", true)],
    );
    let hide = row_of(&w, "t1-2").unwrap();
    w.ctl.borrow_mut().hide_remote(&hide);
    assert_eq!(w.ctl.borrow().hidden_of("mac"), ["t1-2"]);
    w.ctl.borrow_mut().on_msg(UiMsg::Machines(Vec::new()));
    w.pump();
    assert!(watches.live().is_empty());
    assert_eq!(row_of(&w, "t1-1"), None);
    assert!(w.ctl.borrow().hidden_of("mac").is_empty());
    assert!(!flare_settings::machine_prefs::import_hidden(&w.prefs).contains_key("mac"));
    assert!(
        watches.closes().is_empty(),
        "a removed machine's rows are never closed"
    );
    // An answer the stopped watch still sent changes nothing.
    heard(&w, vec![entry("t1-1", "idle", true)]);
    assert_eq!(row_of(&w, "t1-1"), None);
}

/// Rule 1295: an unattached row runs no shell here: a working one holds no window close, takes no dictation and has no
/// handle for a plugin's input.
#[test]
fn an_unattached_row_takes_no_dictation_no_plugin_input_and_holds_no_window_close() {
    let w = Wx::new(1.0);
    importing(&w);
    heard(&w, vec![entry("t1-1", "working", true)]);
    let row = row_of(&w, "t1-1").unwrap();
    w.run(100);
    assert!(
        w.ctl.borrow_mut().hub_mut().working(&row),
        "its marks show the host's work"
    );
    assert_eq!(
        w.ctl.borrow_mut().on_close_requested(),
        crate::appctl::CloseDecision::Close,
        "its quit only lets go of nothing"
    );
    let c = w.ctl.borrow();
    assert!(c.handle_of(&row).is_none());
    assert!(!c.dictation_tabs().iter().any(|t| t.id == row));
}

/// Max's pick ("Yes, show all"): a plugin approved with `tabs` sees an imported row like any tab, with the host's
/// Claude session id.
#[test]
fn a_plugin_with_tabs_sees_imported_rows_with_their_session_ids() {
    let w = Wx::new(1.0);
    importing(&w);
    heard(&w, vec![entry("t1-1", "idle", true)]);
    let row = row_of(&w, "t1-1").unwrap();
    w.run(100);
    let facts = w.ctl.borrow().tab_facts();
    let f = facts.iter().find(|f| f.id == row).expect("the row's facts");
    assert_eq!(f.session.as_deref(), Some(SESSION));
    assert_eq!(f.folder, "/Users/max/t1-1");
}

/// Rules 1295 and 206: a question on an imported row rings here once, before it attaches; attaching it does not ring
/// the same question again.
#[test]
fn a_question_on_an_imported_row_rings_here_once_and_attaching_does_not_ring_it_again() {
    use flare_surfaces::notify::Toast;
    let w = Wx::new(1.0);
    flare_settings::agent_prefs::set_notify_ask_on(&w.prefs, true);
    let got: Rc<RefCell<Vec<Toast>>> = Rc::default();
    let sink = Rc::clone(&got);
    w.ctl
        .borrow_mut()
        .set_toast_sink(Rc::new(move |t: &Toast| sink.borrow_mut().push(t.clone())));
    importing(&w);
    let mut asks = entry("t1-1", "question", true);
    asks["question_text"] = json!("Which colour?");
    heard(&w, vec![asks.clone()]);
    let row = row_of(&w, "t1-1").unwrap();
    w.run(300);
    let rung = |row: &str| got.borrow().iter().filter(|t| t.tab == row).count();
    assert_eq!(rung(&row), 1, "it rings here, unattached");
    pick(&w, &row);
    w.pump_until("the attach", |w| !w.remote.made().is_empty());
    assert!(attached(&w)[0].ring_both, "and on the host too");
    // The attached row's own link brings the host's status: its first answer after the attach is always news (rule
    // 294), and the same question is not rung again.
    let st = crate::remote_status::status_from(&asks).expect("a status");
    w.ctl
        .borrow()
        .model()
        .remote_live(&row)
        .expect("the row's live state")
        .set_status(st);
    w.ctl.borrow_mut().on_msg(UiMsg::HostWake);
    w.run(300);
    // A poll the watch still sent changes nothing for an attached row.
    heard(&w, vec![asks]);
    w.run(300);
    assert_eq!(rung(&row), 1, "the attach does not ring it again");
}
