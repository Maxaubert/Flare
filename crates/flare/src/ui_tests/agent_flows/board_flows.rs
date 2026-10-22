//! Stage 7 through the real window controller (Surfaces): the Preview chip, the agent board and the question
//! notifications. A tab's shell is a fake that "prints" what a dev server or an agent would; the listening table and
//! Windows' notifications are stand-ins the tests own.
//!
//! Held: a local address in a tab's output offers a chip once something listens on its port, opens the web panel at that
//! address on a click, and goes when the listener is gone or the screen was cleared; only a local address ever
//! qualifies; one chip per tab (the newest); the chip belongs to the tab in front; the setting turns it off. The board
//! lists the tabs with an agent, each with its state, time and question, in the strip's order, a click goes to the tab,
//! and it costs no timer while closed. A question out of sight notifies once with the tab's name and its text; none
//! while the tab is in view in a window with the keyboard; the setting turns it off; a click brings the tab up.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flare_settings::agent_prefs::set_notify_ask_on;
use flare_settings::term_prefs::set_dev_preview;
use flare_surfaces::PanelKind;
use flare_surfaces::board::BoardState;
use flare_surfaces::notify::Toast;
use serde_json::json;

use super::*;
use crate::ctl_app::{CtlOp, CtlRequest, CtlRequestBox};

type Listening = Arc<Mutex<Vec<u16>>>;

/// The ports the test says have a listener.
fn listening(f: &Fx) -> Listening {
    let l: Listening = Arc::default();
    let seen = Arc::clone(&l);
    f.ctl
        .borrow_mut()
        .set_port_probe(Arc::new(move |p| seen.lock().unwrap().contains(&p)));
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
fn clicking_the_chip_opens_the_web_panel_at_that_address() {
    let f = Fx::new();
    let ports = listening(&f);
    let id = f.tabs(&["C:\\a"]).remove(0);
    f.visit(0);
    ports.lock().unwrap().push(3000);
    say_url(&f, "C:\\a", "http://127.0.0.1:3000/app?x=1");
    wait_for(&f, "the chip", |f| label(f) == "127.0.0.1:3000");
    f.h.comp.invoke_preview_chip_clicked();
    f.pump();
    let snap = f.ctl.borrow().panel_snap(&id).expect("a panel");
    assert_eq!(snap.kind, "web");
    assert!(snap.open);
    assert_eq!(snap.url.as_deref(), Some("http://127.0.0.1:3000/app?x=1"));
    assert_eq!(
        label(&f),
        "",
        "the panel shows it now: nothing left to offer"
    );
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
    f.pump();
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
    f.ctl.borrow_mut().set_port_probe(Arc::new(|_| true));
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

// ---- the agent board ----------------------------------------------------------------------------------------

fn rows(f: &Fx) -> Vec<flare_surfaces::board::BoardRow> {
    f.ctl.borrow().board_rows()
}

fn open_board(f: &Fx) {
    assert!(f.ctl.borrow_mut().toggle_board());
    f.pump();
}

#[test]
fn the_board_lists_only_tabs_with_an_agent_in_the_strips_order() {
    let f = Fx::new();
    f.tabs(&["C:\\a", "C:\\b", "C:\\c"]);
    claude(&f, "C:\\c");
    claude(&f, "C:\\a");
    f.visit(1);
    open_board(&f);
    let r = rows(&f);
    let names: Vec<&str> = r.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        ["a", "c"],
        "tab b has no agent; the strip's order, not the order they began"
    );
    assert_eq!(f.h.comp.get_panel_kind(), 6);
    assert_eq!(f.h.comp.get_panel_title(), "Agents");
    assert_eq!(f.h.comp.get_panel_sub(), "2 agents");
    assert_eq!(f.h.comp.get_panel_board_row_count(), 2);
    assert!(r.iter().all(|r| r.agent == "Claude"));
}

#[test]
fn every_state_is_worded_and_the_time_is_the_states_own() {
    let f = Fx::new();
    let ids = f.tabs(&["C:\\a", "C:\\b", "C:\\c", "C:\\d"]);
    for c in ["C:\\a", "C:\\b", "C:\\c", "C:\\d"] {
        claude(&f, c);
    }
    f.say("C:\\a", W);
    f.say("C:\\b", Q);
    f.say("C:\\c", D);
    f.say("C:\\d", F);
    f.pump_until("the marks", |f| {
        let mut c = f.ctl.borrow_mut();
        c.hub_mut().working(&ids[0])
    });
    f.step(65_000);
    open_board(&f);
    let r = rows(&f);
    let by = |n: &str| r.iter().find(|r| r.name == n).unwrap().clone();
    assert_eq!(by("a").state, BoardState::Working);
    assert!(
        by("a").status.starts_with("Working for 1m"),
        "{}",
        by("a").status
    );
    assert_eq!(by("b").state, BoardState::Question);
    assert!(
        by("b").status.starts_with("Waiting 1m"),
        "{}",
        by("b").status
    );
    assert_eq!(by("c").state, BoardState::Finished);
    assert!(
        by("c").status.starts_with("Finished 1m"),
        "{}",
        by("c").status
    );
    assert_eq!(by("d").state, BoardState::Failed);
    assert!(
        by("d").status.starts_with("Failed 1m"),
        "{}",
        by("d").status
    );
    assert_eq!(by("d").detail, "rate limit");
    // The clocks move while it is open.
    f.step(30_000);
    f.step(1_000);
    let a = rows(&f).into_iter().find(|r| r.name == "a").unwrap();
    assert!(
        a.status.starts_with("Working for 1m 3") || a.status.starts_with("Working for 1m 4"),
        "{}",
        a.status
    );
}

#[test]
fn an_agent_with_nothing_to_say_is_idle() {
    let f = Fx::new();
    f.tabs(&["C:\\a"]);
    claude(&f, "C:\\a");
    open_board(&f);
    let r = rows(&f);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].state, BoardState::Idle);
    assert_eq!(r[0].status, "Idle");
}

#[test]
fn a_question_row_shows_the_question_when_the_screen_has_it() {
    let f = Fx::new();
    f.tabs(&["C:\\a"]);
    f.visit(0);
    claude(&f, "C:\\a");
    f.say("C:\\a", Q);
    f.say("C:\\a", &paint_owned(&screens("run2-01-question-1")));
    f.step(150);
    f.step(0);
    open_board(&f);
    let r = rows(&f);
    assert_eq!(r[0].state, BoardState::Question);
    assert_eq!(r[0].detail, "Pick a colour?");
    let ui = f.h.comp.get_panel_board_rows().row_data(0).unwrap();
    assert_eq!(ui.detail, "Pick a colour?");
    assert_eq!(ui.state, 2);
}

#[test]
fn a_click_on_a_row_goes_to_that_tab() {
    let f = Fx::new();
    let ids = f.tabs(&["C:\\a", "C:\\b"]);
    claude(&f, "C:\\a");
    claude(&f, "C:\\b");
    f.visit(1);
    open_board(&f);
    f.h.comp.invoke_panel_board_pick(ids[0].as_str().into());
    f.pump();
    assert_eq!(f.front(), ids[0]);
    // The board belongs to tab b's panel; tab a shows its own (none).
    assert_eq!(f.h.comp.get_panel_kind(), 0);
    // A tab that does not exist is ignored.
    f.h.comp.invoke_panel_board_pick("t-nope".into());
    f.pump();
    assert_eq!(f.front(), ids[0]);
}

#[test]
fn the_board_costs_no_timer_and_no_rows_while_closed() {
    let f = Fx::new();
    f.tabs(&["C:\\a"]);
    claude(&f, "C:\\a");
    f.visit(0);
    f.step(100);
    assert_eq!(probe(&f)["board_timer"], false);
    assert_eq!(f.h.comp.get_panel_board_row_count(), 0);
    open_board(&f);
    assert_eq!(probe(&f)["board_timer"], true);
    assert_eq!(f.h.comp.get_panel_board_row_count(), 1);
    // Closing it lets go of both.
    assert!(f.ctl.borrow_mut().toggle_board());
    f.pump();
    assert_eq!(probe(&f)["board_timer"], false);
    assert_eq!(f.h.comp.get_panel_board_row_count(), 0);
    assert_eq!(f.h.comp.get_panel_kind(), 0);
}

#[test]
fn the_chord_opens_and_closes_the_board_and_the_shell_never_hears_it() {
    let f = Fx::new();
    f.tabs(&["C:\\a"]);
    claude(&f, "C:\\a");
    f.visit(0);
    let chord = KeyEvent::new("A", "KeyA").ctrl().shift();
    assert_eq!(f.key(chord.clone()), KeyOutcome::Consumed);
    assert_eq!(f.h.comp.get_panel_kind(), 6);
    assert!(f.h.comp.get_panel_shown());
    assert_eq!(f.key(chord), KeyOutcome::Consumed);
    assert!(!f.h.comp.get_panel_shown());
    // Plain Ctrl+A is the shell's.
    assert_eq!(f.key(KeyEvent::new("a", "KeyA").ctrl()), KeyOutcome::Pty);
}

#[test]
fn an_empty_board_says_so() {
    let f = Fx::new();
    f.tabs(&["C:\\a"]);
    f.visit(0);
    open_board(&f);
    assert_eq!(
        f.h.comp.get_panel_note(),
        "No agent is running in your tabs."
    );
    assert_eq!(f.h.comp.get_panel_board_row_count(), 0);
}

#[test]
fn the_terminal_menu_gains_one_row_that_follows_the_board() {
    let f = Fx::new();
    let id = f.tabs(&["C:\\a"]).remove(0);
    f.visit(0);
    assert!(
        f.ctl.borrow().board_menu_rows(&id).is_empty(),
        "no agent anywhere: no row"
    );
    claude(&f, "C:\\a");
    let row = f.ctl.borrow().board_menu_rows(&id);
    assert_eq!(row.len(), 1);
    assert_eq!(row[0].0.label, "Show agent board");
    assert_eq!(row[0].0.hint, "Ctrl+Shift+A");
    assert!(
        f.ctl
            .borrow_mut()
            .board_menu_picked(&id, "Show agent board")
    );
    f.pump();
    assert_eq!(f.h.comp.get_panel_kind(), 6);
    let row = f.ctl.borrow().board_menu_rows(&id);
    assert_eq!(row[0].0.label, "Hide agent board");
    assert!(
        f.ctl
            .borrow_mut()
            .board_menu_picked(&id, "Hide agent board")
    );
    f.pump();
    assert!(!f.h.comp.get_panel_shown());
    assert!(!f.ctl.borrow_mut().board_menu_picked(&id, "Copy"));
}

#[test]
fn the_board_panel_is_stored_like_another_and_comes_back_with_the_tab() {
    let f = Fx::new();
    let id = f.tabs(&["C:\\a"]).remove(0);
    assert!(f.ctl.borrow_mut().open_convo_panel(&id, PanelKind::Board));
    let snap = f.ctl.borrow().panel_snap(&id).unwrap();
    assert_eq!(snap.kind, "board");
    assert_eq!(snap.title, "Agents");
    assert_eq!(snap.path, None);
    assert!(snap.open);
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
fn the_hub_keeps_when_each_state_began_and_forgets_a_closed_tab() {
    let f = Fx::new();
    let id = f.tabs(&["C:\\a"]).remove(0);
    claude(&f, "C:\\a");
    f.say("C:\\a", W);
    f.pump_until("working", |f| f.ctl.borrow_mut().hub_mut().working(&id));
    f.step(10_000);
    let e = f.ctl.borrow().hub().board_entry(&id).unwrap();
    assert_eq!(e.state, BoardState::Working);
    assert!(e.in_state_ms.unwrap() >= 10_000);
    f.ctl.borrow_mut().close_now(&id);
    f.pump();
    assert!(f.ctl.borrow().hub().board_entry(&id).is_none());
}

// ---- the control channel ----

#[test]
fn panel_open_board_opens_it_on_the_agents_own_tab_and_switches_no_tab() {
    let f = Fx::new();
    let ids = f.tabs(&["C:\\a", "C:\\b"]);
    claude(&f, "C:\\a");
    f.visit(1);
    let (tx, rx) = std::sync::mpsc::channel();
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Ctl(CtlRequestBox(CtlRequest {
            op: CtlOp::ShowConvo {
                tab: ids[0].clone(),
                kind: PanelKind::Board,
            },
            deadline: Instant::now() + Duration::from_secs(5),
            reply: tx,
        })));
    let v = rx
        .recv_timeout(Duration::from_secs(5)) // sync-ok: test code only
        .expect("an answer")
        .expect("not an error");
    assert_eq!(v["panel"], json!({"kind": "board", "title": "Agents"}));
    f.pump();
    assert_eq!(
        f.front(),
        ids[1],
        "the agent's call never switches the person's tab"
    );
    assert_eq!(f.h.comp.get_panel_kind(), 0, "tab b shows no panel");
    f.visit(0);
    assert_eq!(f.h.comp.get_panel_kind(), 6, "tab a's panel is the board");
    assert_eq!(f.h.comp.get_panel_board_row_count(), 1);
}
