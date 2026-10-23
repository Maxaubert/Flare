//! Stage 7 through the real window controller (Surfaces): the Preview chip and the question
//! notifications. A tab's shell is a fake that "prints" what a dev server or an agent would; the listening table and
//! Windows' notifications are stand-ins the tests own.
//!
//! Held: a local address in a tab's output offers a chip once something listens on its port, opens the web panel at that
//! address on a click, and goes when the listener is gone or the screen was cleared; only a local address ever
//! qualifies; one chip per tab (the newest); the chip belongs to the tab in front; the setting turns it off. A question out of sight notifies once with the tab's name and its text; none
//! while the tab is in view in a window with the keyboard; the setting turns it off; a click brings the tab up.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_settings::agent_prefs::set_notify_ask_on;
use flare_settings::term_prefs::set_dev_preview;
use flare_surfaces::devpreview::PortLook;
use flare_surfaces::notify::Toast;

use super::*;

type Listening = Arc<Mutex<Vec<u16>>>;

/// The ports the test says have a listener.
fn listening(f: &Fx) -> Listening {
    let l: Listening = Arc::default();
    let seen = Arc::clone(&l);
    f.ctl.borrow_mut().set_port_probe(Arc::new(move |p, _| {
        PortLook::from(seen.lock().unwrap().contains(&p))
    }));
    l
}

fn label(f: &Fx) -> String {
    f.h.comp.get_preview_chip_label().to_string()
}

fn probe(f: &Fx) -> serde_json::Value {
    f.ctl.borrow().surf7_probe()
}

/// Steps the window in 50 ms slices of fake time, with a breath of real time between them for the workers, until `cond`.
fn wait_for(f: &Fx, what: &str, mut cond: impl FnMut(&Fx) -> bool) {
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

/// A few steps with the real time for a worker to answer, for "nothing happens" checks.
fn settle(f: &Fx) {
    for _ in 0..12 {
        f.step(100);
        std::thread::sleep(Duration::from_millis(3));
    }
}

fn say_url(f: &Fx, cwd: &str, url: &str) {
    f.say(cwd, &format!("  Local:   {url}\r\n"));
}

fn toasts(f: &Fx) -> Rc<RefCell<Vec<Toast>>> {
    let got: Rc<RefCell<Vec<Toast>>> = Rc::default();
    let sink = Rc::clone(&got);
    f.ctl
        .borrow_mut()
        .set_toast_sink(Rc::new(move |t: &Toast| sink.borrow_mut().push(t.clone())));
    got
}

/// A Claude tab in `cwd`: the agent is known and idle.
fn claude(f: &Fx, cwd: &str) {
    f.say(cwd, &title(T_IDLE));
    f.pump_until("the agent", |f| {
        let id = f.ids()[f.cwds().iter().position(|c| c == cwd).unwrap()].clone();
        f.ctl.borrow_mut().hub_mut().agent_here(&id)
    });
}

// ---- the Preview chip ---------------------------------------------------------------------------------------

#[test]
fn a_local_address_offers_a_chip_once_its_port_listens() {
    let f = Fx::new();
    let ports = listening(&f);
    f.tabs(&["C:\\a"]);
    f.visit(0);
    say_url(&f, "C:\\a", "http://localhost:5173/");
    settle(&f);
    assert_eq!(label(&f), "", "nothing listens yet, no chip");
    ports.lock().unwrap().push(5173);
    wait_for(&f, "the chip", |f| label(f) == "localhost:5173");
    assert_eq!(probe(&f)["chip"]["url"], "http://localhost:5173/");
    assert_eq!(probe(&f)["chip"]["port"], 5173);
}

#[test]
fn the_chip_shows_exactly_the_origin_and_drops_path_and_query() {
    let f = Fx::new();
    let ports = listening(&f);
    f.tabs(&["C:\\a"]);
    f.visit(0);
    ports.lock().unwrap().push(5173);
    say_url(&f, "C:\\a", "http://localhost:5173/admin/delete?id=1#x");
    wait_for(&f, "the chip", |f| label(f) == "localhost:5173");
    assert_eq!(probe(&f)["chip"]["url"], "http://localhost:5173/");
    assert_eq!(probe(&f)["chip_tip"], "http://localhost:5173/");
}

#[test]
fn the_port_is_looked_up_against_the_tabs_own_shell() {
    let f = Fx::new();
    let asked: Arc<Mutex<Vec<(u16, u32)>>> = Arc::default();
    let seen = Arc::clone(&asked);
    // Only a port owned by pid 0 would "listen"; a real shell pid is handed over, so this never shows a chip.
    f.ctl.borrow_mut().set_port_probe(Arc::new(move |p, pid| {
        seen.lock().unwrap().push((p, pid));
        PortLook::Nobody
    }));
    f.tabs(&["C:\\a"]);
    f.visit(0);
    say_url(&f, "C:\\a", "http://localhost:5173/");
    wait_for(&f, "a look", |_| !asked.lock().unwrap().is_empty());
    let (port, pid) = asked.lock().unwrap()[0];
    assert_eq!(port, 5173);
    let shells = f.ctl.borrow().deps_shell_pids();
    assert!(
        shells.contains(&pid),
        "the look names the tab's shell: {pid} in {shells:?}"
    );
    assert_eq!(label(&f), "", "a port nobody in the tab owns gives no chip");
}

#[test]
fn clicking_the_chip_opens_the_web_panel_at_that_address() {
    let f = Fx::new();
    let ports = listening(&f);
    let id = f.tabs(&["C:\\a"]).remove(0);
    f.visit(0);
    ports.lock().unwrap().push(3000);
    say_url(&f, "C:\\a", "http://127.0.0.1:3000/app?x=1");
    wait_for(&f, "the chip", |f| label(f) == "127.0.0.1:3000");
    f.h.comp.invoke_preview_chip_clicked();
    wait_for(&f, "the panel", |f| {
        f.ctl.borrow().panel_snap(&id).is_some_and(|s| s.open)
    });
    let snap = f.ctl.borrow().panel_snap(&id).expect("a panel");
    assert_eq!(snap.kind, "web");
    assert!(snap.open);
    assert_eq!(
        snap.url.as_deref(),
        Some("http://127.0.0.1:3000/"),
        "only the origin opens, never the printed path or query"
    );
    assert_eq!(
        label(&f),
        "",
        "the panel shows it now: nothing left to offer"
    );
}

#[test]
fn the_chip_stays_away_while_the_panel_shows_any_page_of_that_server() {
    let f = Fx::new();
    let ports = listening(&f);
    let id = f.tabs(&["C:\\a"]).remove(0);
    f.visit(0);
    ports.lock().unwrap().push(5173);
    say_url(&f, "C:\\a", "http://localhost:5173/");
    wait_for(&f, "the chip", |f| label(f) == "localhost:5173");
    f.h.comp.invoke_preview_chip_clicked();
    wait_for(&f, "the panel", |f| {
        f.ctl.borrow().panel_snap(&id).is_some_and(|s| s.open)
    });
    assert_eq!(label(&f), "");
    // The person follows a link inside the app: the panel is on /settings, and the chip does not come back (a
    // click on it would send the page back to the origin and lose the place).
    f.ctl
        .borrow_mut()
        .follow_link_in_panel(&id, "http://localhost:5173/settings");
    f.pump();
    assert_eq!(label(&f), "", "the panel still shows that server");
}

#[test]
fn a_chip_for_a_bare_host_opens_the_normal_form() {
    let f = Fx::new();
    let ports = listening(&f);
    let id = f.tabs(&["C:\\a"]).remove(0);
    f.visit(0);
    ports.lock().unwrap().push(8080);
    say_url(&f, "C:\\a", "HTTP://LocalHost:8080");
    wait_for(&f, "the chip", |f| label(f) == "localhost:8080");
    f.h.comp.invoke_preview_chip_clicked();
    wait_for(&f, "the panel", |f| {
        f.ctl.borrow().panel_snap(&id).is_some_and(|s| s.open)
    });
    let snap = f.ctl.borrow().panel_snap(&id).expect("a panel");
    assert_eq!(snap.url.as_deref(), Some("http://localhost:8080/"));
}

#[test]
fn the_chip_goes_when_the_process_that_printed_it_is_gone() {
    let f = Fx::new();
    let ports = listening(&f);
    f.tabs(&["C:\\a"]);
    f.visit(0);
    ports.lock().unwrap().push(5173);
    say_url(&f, "C:\\a", "http://localhost:5173/");
    wait_for(&f, "the chip", |f| label(f) == "localhost:5173");
    // The server stops: two failed looks in a row take the chip.
    ports.lock().unwrap().clear();
    wait_for(&f, "the chip to go", |f| label(f).is_empty());
    assert_eq!(probe(&f)["tracked"], 0);
}

#[test]
fn clearing_the_screen_takes_the_chip_at_once() {
    let f = Fx::new();
    let ports = listening(&f);
    f.tabs(&["C:\\a"]);
    f.visit(0);
    ports.lock().unwrap().push(5173);
    say_url(&f, "C:\\a", "http://localhost:5173/");
    wait_for(&f, "the chip", |f| label(f) == "localhost:5173");
    f.say("C:\\a", "\x1b[2J\x1b[3J\x1b[H");
    f.step(0);
    assert_eq!(
        label(&f),
        "",
        "the clear took it, though the server still listens"
    );
    settle(&f);
    assert_eq!(label(&f), "");
}

#[test]
fn a_server_started_again_on_the_same_port_gets_its_chip_back() {
    let f = Fx::new();
    let ports = listening(&f);
    f.tabs(&["C:\\a"]);
    f.visit(0);
    ports.lock().unwrap().push(3000);
    say_url(&f, "C:\\a", "http://localhost:3000/");
    wait_for(&f, "the chip", |f| label(f) == "localhost:3000");
    // Ctrl+C: the listener goes and so does the chip; the screen is not cleared.
    ports.lock().unwrap().clear();
    wait_for(&f, "the chip to go", |f| label(f).is_empty());
    // Started again: the same address, printed a second time.
    ports.lock().unwrap().push(3000);
    f.say("C:\\a", "^C\r\nrestarting\r\n");
    say_url(&f, "C:\\a", "http://localhost:3000/");
    wait_for(&f, "the chip to come back", |f| {
        label(f) == "localhost:3000"
    });
}

#[test]
fn another_tabs_steady_output_never_holds_back_a_chip() {
    let f = Fx::new();
    let ports = listening(&f);
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    ports.lock().unwrap().push(5173);
    // The banner arrives inside the terminal's 40 ms gap after a first reading, so its reading is owed.
    f.say("C:\\a", "starting\r\n");
    say_url(&f, "C:\\a", "http://localhost:5173/");
    let mut shown = false;
    for i in 0..60 {
        f.say("C:\\b", &format!("tick {i}\r\n"));
        f.step(10);
        std::thread::sleep(Duration::from_millis(5));
        if label(&f) == "localhost:5173" {
            shown = true;
            break;
        }
    }
    assert!(shown, "tab a's chip appeared while tab b kept printing");
}

#[test]
fn the_newest_address_is_the_only_chip_of_a_tab() {
    let f = Fx::new();
    let ports = listening(&f);
    f.tabs(&["C:\\a"]);
    f.visit(0);
    ports.lock().unwrap().extend([3000, 4000]);
    say_url(&f, "C:\\a", "http://localhost:3000/");
    wait_for(&f, "the first chip", |f| label(f) == "localhost:3000");
    say_url(&f, "C:\\a", "http://localhost:4000/");
    wait_for(&f, "the second chip", |f| label(f) == "localhost:4000");
    assert_eq!(probe(&f)["tracked"], 1);
}

#[test]
fn an_address_that_is_not_local_never_gets_a_chip() {
    let f = Fx::new();
    // Everything "listens": only the address can keep the chip away.
    f.ctl
        .borrow_mut()
        .set_port_probe(Arc::new(|_, _| PortLook::InTree));
    f.tabs(&["C:\\a"]);
    f.visit(0);
    for url in [
        "https://example.com:3000/",
        "http://localhost.evil.com:3000/",
        "http://127.0.0.1.nip.io:3000/",
        "http://localhost:3000@evil.com/",
        "http://0.0.0.0:3000/",
        "http://192.168.1.2:3000/",
        "http://localhost/",
        "file:///C:/x.html",
    ] {
        say_url(&f, "C:\\a", url);
    }
    settle(&f);
    assert_eq!(label(&f), "");
    assert_eq!(probe(&f)["tracked"], 0, "not even looked at");
}

#[test]
fn colour_around_the_address_is_one_address() {
    let f = Fx::new();
    let ports = listening(&f);
    f.tabs(&["C:\\a"]);
    f.visit(0);
    ports.lock().unwrap().push(5173);
    f.say(
        "C:\\a",
        "  \x1b[32m\u{279c}\x1b[39m  \x1b[1mLocal\x1b[22m:   \x1b[36mhttp://localhost:\x1b[1m5173\x1b[22m/\x1b[39m\r\n",
    );
    wait_for(&f, "the chip", |f| label(f) == "localhost:5173");
}

#[test]
fn the_chip_belongs_to_the_tab_in_front() {
    let f = Fx::new();
    let ports = listening(&f);
    f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(1);
    ports.lock().unwrap().push(5173);
    say_url(&f, "C:\\a", "http://localhost:5173/");
    wait_for(&f, "the look at the port", |f| probe(f)["tracked"] == 1);
    settle(&f);
    assert_eq!(label(&f), "", "tab b is in front; the server is tab a's");
    f.visit(0);
    wait_for(&f, "the chip", |f| label(f) == "localhost:5173");
    f.visit(1);
    assert_eq!(label(&f), "");
}

#[test]
fn the_setting_turns_the_chips_off_and_on() {
    let f = Fx::new();
    let ports = listening(&f);
    f.tabs(&["C:\\a"]);
    f.visit(0);
    ports.lock().unwrap().push(5173);
    say_url(&f, "C:\\a", "http://localhost:5173/");
    wait_for(&f, "the chip", |f| label(f) == "localhost:5173");
    set_dev_preview(&f.prefs, false);
    f.pump();
    f.step(0);
    assert_eq!(label(&f), "", "off: the chip goes at once");
    say_url(&f, "C:\\a", "http://localhost:5174/");
    ports.lock().unwrap().push(5174);
    settle(&f);
    assert_eq!(label(&f), "", "off: nothing new is offered");
    set_dev_preview(&f.prefs, true);
    wait_for(&f, "the chip to come back", |f| {
        label(f) == "localhost:5174"
    });
}

#[test]
fn a_closed_tab_forgets_its_chip() {
    let f = Fx::new();
    let ports = listening(&f);
    let ids = f.tabs(&["C:\\a", "C:\\b"]);
    f.visit(0);
    ports.lock().unwrap().push(5173);
    say_url(&f, "C:\\a", "http://localhost:5173/");
    wait_for(&f, "the chip", |f| label(f) == "localhost:5173");
    f.ctl.borrow_mut().close_now(&ids[0]);
    f.pump();
    assert_eq!(probe(&f)["tracked"], 0);
    assert_eq!(label(&f), "");
}

#[test]
fn quiet_output_costs_nothing_here() {
    let f = Fx::new();
    f.tabs(&["C:\\a"]);
    f.visit(0);
    for i in 0..200 {
        f.say(
            "C:\\a",
            &format!("line {i} of a build that names no server\r\n"),
        );
    }
    // The terminal reads its screen at most every 40 ms of real time; what it owed is read, and then nothing is due.
    wait_for(&f, "the screen to be read", |f| {
        f.ctl.borrow().surf7_next_due().is_none()
    });
    assert_eq!(probe(&f)["tracked"], 0);
}

// ---- questions as notifications ---------------------------------------------------------------------------

#[test]
fn a_question_in_a_tab_out_of_sight_notifies_once_with_the_tabs_name_and_text() {
    let f = Fx::new();
    let got = toasts(&f);
    let ids = f.tabs(&["C:\\work", "C:\\other"]);
    f.visit(1);
    claude(&f, "C:\\work");
    f.say("C:\\work", Q);
    f.say("C:\\work", &paint_owned(&screens("run2-01-question-1")));
    f.step(150);
    f.step(0);
    wait_for(&f, "the notification", |_| !got.borrow().is_empty());
    for _ in 0..30 {
        f.step(100);
    }
    let all = got.borrow().clone();
    assert_eq!(all.len(), 1, "one question, one notification: {all:?}");
    assert_eq!(all[0].tab, ids[0]);
    assert_eq!(all[0].title, "work");
    assert_eq!(all[0].body, "Pick a colour?");
}

#[test]
fn a_question_without_its_text_waits_out_the_grace_and_says_who_asks() {
    let f = Fx::new();
    let got = toasts(&f);
    f.tabs(&["C:\\work", "C:\\other"]);
    f.visit(1);
    claude(&f, "C:\\work");
    f.say("C:\\work", Q);
    f.step(100);
    assert!(got.borrow().is_empty(), "the grace is not over");
    f.step(600);
    let all = got.borrow().clone();
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0].body, "Claude has a question for you.");
}

#[test]
fn nothing_while_the_person_is_looking_at_that_tab() {
    let f = Fx::new();
    let got = toasts(&f);
    f.tabs(&["C:\\work"]);
    f.visit(0);
    claude(&f, "C:\\work");
    f.say("C:\\work", Q);
    f.step(100);
    f.step(1_000);
    f.step(5_000);
    assert!(got.borrow().is_empty(), "{:?}", got.borrow());
}

#[test]
fn an_unfocused_window_notifies_even_for_the_tab_in_front() {
    let f = Fx::new();
    let got = toasts(&f);
    f.tabs(&["C:\\work"]);
    f.visit(0);
    claude(&f, "C:\\work");
    f.ctl.borrow_mut().on_focus(false);
    f.say("C:\\work", Q);
    f.step(700);
    assert_eq!(got.borrow().len(), 1);
}

#[test]
fn coming_back_to_the_window_before_the_grace_ends_cancels_it() {
    let f = Fx::new();
    let got = toasts(&f);
    f.tabs(&["C:\\work"]);
    f.visit(0);
    claude(&f, "C:\\work");
    f.ctl.borrow_mut().on_focus(false);
    f.say("C:\\work", Q);
    f.step(200);
    f.ctl.borrow_mut().on_focus(true);
    f.step(2_000);
    assert!(got.borrow().is_empty());
}

#[test]
fn the_setting_turns_the_notification_off() {
    let f = Fx::new();
    let got = toasts(&f);
    set_notify_ask_on(&f.prefs, false);
    f.tabs(&["C:\\work", "C:\\other"]);
    f.visit(1);
    claude(&f, "C:\\work");
    f.say("C:\\work", Q);
    f.step(100);
    f.step(2_000);
    assert!(got.borrow().is_empty());
}

#[test]
fn a_second_question_in_the_same_tab_is_a_second_notification() {
    let f = Fx::new();
    let got = toasts(&f);
    f.tabs(&["C:\\work", "C:\\other"]);
    f.visit(1);
    claude(&f, "C:\\work");
    f.say("C:\\work", Q);
    f.step(700);
    assert_eq!(got.borrow().len(), 1);
    // The agent works again, then asks again.
    f.say("C:\\work", W);
    f.step(300);
    f.say("C:\\work", Q);
    f.step(700);
    assert_eq!(got.borrow().len(), 2);
}

#[test]
fn a_click_on_the_notification_brings_the_tab_to_the_front() {
    let f = Fx::new();
    let got = toasts(&f);
    let ids = f.tabs(&["C:\\work", "C:\\other"]);
    f.visit(1);
    claude(&f, "C:\\work");
    f.say("C:\\work", Q);
    f.step(700);
    let tab = got.borrow()[0].tab.clone();
    // The window's raise is a stand-in in the tests; the pick is what is held.
    f.ctl.borrow_mut().on_msg(UiMsg::ToastClicked(tab.clone()));
    f.pump();
    assert_eq!(f.front(), ids[0]);
    // A notification for a tab that has since closed does nothing.
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::ToastClicked("t-gone".into()));
    f.pump();
    assert_eq!(f.front(), ids[0]);
}

#[test]
fn a_notification_windows_would_not_take_flashes_the_taskbar_instead() {
    let f = Fx::new();
    let flashes = Rc::new(Cell::new(0));
    {
        let n = Rc::clone(&flashes);
        f.ctl
            .borrow_mut()
            .set_flash(Rc::new(move || n.set(n.get() + 1)));
    }
    f.ctl.borrow_mut().on_msg(UiMsg::ToastFailed);
    assert_eq!(flashes.get(), 1);
    // With no sink at all (a build that never installed one) a question flashes too.
    f.tabs(&["C:\\work", "C:\\other"]);
    f.visit(1);
    claude(&f, "C:\\work");
    f.say("C:\\work", Q);
    f.step(700);
    assert_eq!(flashes.get(), 2);
}

#[test]
fn a_click_looks_again_and_opens_nothing_when_the_port_changed_hands() {
    let f = Fx::new();
    let state: Arc<Mutex<PortLook>> = Arc::new(Mutex::new(PortLook::InTree));
    let seen = Arc::clone(&state);
    f.ctl
        .borrow_mut()
        .set_port_probe(Arc::new(move |_, _| *seen.lock().unwrap()));
    let id = f.tabs(&["C:\\a"]).remove(0);
    f.visit(0);
    say_url(&f, "C:\\a", "http://localhost:3000/");
    wait_for(&f, "the chip", |f| label(f) == "localhost:3000");
    // The server exits and a stranger binds the port before the next routine look.
    *state.lock().unwrap() = PortLook::Foreign;
    f.h.comp.invoke_preview_chip_clicked();
    wait_for(&f, "the chip to go", |f| label(f).is_empty());
    settle(&f);
    let open = f.ctl.borrow().panel_snap(&id).is_some_and(|s| s.open);
    assert!(!open, "a foreign server's page never opens");
    assert_eq!(probe(&f)["tracked"], 0, "the chip is gone, not on a grace");
}
