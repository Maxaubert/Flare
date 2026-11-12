//! The page drawn in the web panel (macOS, CEF) has its own keys, as WebView2's window has on Windows: every key goes
//! to the page while it has them and its key filter hands the app's chords and the second Escape back (KEY-01,
//! KEY-02); the focus chord and a press move them (KEY-03); the history keys and buttons (NAV-06); the zoom keys
//! (NAT-05); Flare's find bar over the page (NAT-02); the status text and the input method (NAT-09). The browser is
//! the fake; the filter is the one the app hands it.

use flare_tabs::keymap::KeyContext;
use flare_web::fake::{Call, FakeWeb};
use flare_web::{KeyPress, Nav, WebEvent};

use super::*;
use crate::ctl_app::CtlOp;

const PAGE: &str = "https://flare-test.local/index.html";
const TWO: &str = "https://flare-test.local/two.html";
/// Slint's `control` is Cmd on a Mac (rule 1110): the panel's bit 2 there, 8 elsewhere.
const CMD_BIT: i32 = if cfg!(target_vendor = "apple") { 2 } else { 8 };

type Slot = Rc<RefCell<Option<FakeWeb>>>;

fn fake(slot: &Slot) -> FakeWeb {
    slot.borrow().clone().expect("the browser was made")
}

/// A Mac window with a live page in front (the fake draws a picture), loaded, the history given.
fn mac_page() -> (Fx, Slot, String) {
    let (f, _h, _) = live(Opts::default());
    f.ctl.borrow_mut().set_key_os(KeyOs::Mac);
    let slot: Slot = Rc::default();
    let keep = slot.clone();
    f.ctl.borrow_mut().set_web_factory(Box::new(move |sink| {
        let fake = FakeWeb::new(sink);
        *keep.borrow_mut() = Some(fake.clone());
        Box::new(fake)
    }));
    let id = f.ctl.borrow().front_id().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Ctl(crate::ctl_app::CtlRequestBox(
            crate::ctl_app::CtlRequest {
                op: CtlOp::BrowserOpen {
                    tab: id.clone(),
                    url: PAGE.into(),
                },
                deadline: Instant::now() + Duration::from_secs(5),
                reply: tx,
            },
        )));
    load(&fake(&slot), &id, PAGE, 1);
    f.pump();
    assert!(rx.try_recv().unwrap().is_ok());
    fake(&slot).state.borrow_mut().frame = Some(flare_web::ViewFrame {
        width: 4,
        height: 2,
        bgra: std::sync::Arc::new([0x20, 0x20, 0x20, 0xff].repeat(8)),
        serial: 1,
    });
    fake(&slot).say(WebEvent::Pump);
    f.pump();
    f.h.advance(0);
    assert!(
        f.h.comp.get_panel_web_live(),
        "the page is drawn in its box"
    );
    (f, slot, id)
}

fn load(fake: &FakeWeb, tab: &str, url: &str, nav: u64) {
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

/// Cmd+Shift+O: the page takes the keys (the browser's focus and the page's own scope).
fn focus_page(f: &Fx, slot: &Slot, id: &str) {
    assert_eq!(
        f.key(KeyEvent::new("O", "KeyO").meta().shift()),
        KeyOutcome::Consumed
    );
    assert!(fake(slot).calls().contains(&Call::Focus(id.into())));
    fake(slot).say(WebEvent::Focus {
        tab: id.into(),
        has: true,
    });
    f.pump();
    assert!(f.ctl.borrow().web.has_focus());
    assert!(
        f.h.comp.get_panel_web_focused(),
        "the page's own scope has Slint's focus"
    );
    assert!(f.ctl.borrow().page_has_keys());
}

/// What the page's key filter says of a key (true: the app takes it), as the host asks it before the page.
fn filter(slot: &Slot, k: KeyPress) -> bool {
    let filter = fake(slot)
        .state
        .borrow()
        .filter
        .clone()
        .expect("a key filter");
    filter(&k)
}

fn press(vk: u32, cmd: bool, shift: bool) -> KeyPress {
    KeyPress {
        vk,
        ctrl: cmd,
        shift,
        alt: false,
        down: true,
        repeat: false,
    }
}

/// The page handed a key back (the filter took it).
fn handed_back(slot: &Slot, tab: &str, k: KeyPress) {
    fake(slot).say(WebEvent::Key {
        tab: tab.into(),
        key: k,
    });
}

fn zooms(slot: &Slot, tab: &str) -> Vec<f64> {
    fake(slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Zoom(t, z) if t == tab => Some(z),
            _ => None,
        })
        .collect()
}

#[test]
fn a_page_with_the_keys_gets_every_key_and_its_filter_hands_the_apps_chords_back() {
    let (f, slot, id) = mac_page();
    // Before the page has the keys, a plain key is the shell's.
    assert_eq!(f.key(KeyEvent::new("a", "KeyA")), KeyOutcome::Pty);
    focus_page(&f, &slot, &id);
    // Now every key goes on to Slint (the page's scope), never to the shell: plain keys, Escape, Ctrl+C.
    assert_eq!(f.key(KeyEvent::new("a", "KeyA")), KeyOutcome::Slint);
    assert_eq!(f.key(KeyEvent::new("Escape", "Escape")), KeyOutcome::Slint);
    assert_eq!(f.key(KeyEvent::new("c", "KeyC").ctrl()), KeyOutcome::Slint);
    // The filter takes the app's chords (Cmd+T, Cmd+W, the zoom keys, Cmd+F) and leaves the rest to the page.
    assert!(filter(&slot, press(0x54, true, false)), "Cmd+T");
    assert!(filter(&slot, press(0x57, true, false)), "Cmd+W");
    assert!(filter(&slot, press(0xBB, true, false)), "Cmd+=");
    assert!(
        filter(&slot, press(0x46, true, false)),
        "Cmd+F (Flare's find bar)"
    );
    assert!(
        filter(&slot, press(0xDB, true, true)),
        "Cmd+Shift+[ (the tab before)"
    );
    assert!(!filter(&slot, press(0x41, false, false)), "a");
    assert!(
        !filter(&slot, press(0x43, true, false)),
        "Cmd+C is the page's copy"
    );
    assert!(
        !filter(&slot, press(0xDB, true, false)),
        "Cmd+[ is the page's back"
    );
    // A chord handed back runs: Cmd+T makes a tab.
    let before = f.ctl.borrow().model().state.tabs.len();
    handed_back(&slot, &id, press(0x54, true, false));
    f.pump();
    assert_eq!(f.ctl.borrow().model().state.tabs.len(), before + 1);
}

#[test]
fn on_windows_ctrl_f_stays_the_pages_and_on_a_mac_cmd_f_is_flares() {
    let k = press(0x46, true, false);
    let win = KeyContext::default();
    let mac = KeyContext {
        os: KeyOs::Mac,
        ..KeyContext::default()
    };
    assert!(
        !crate::web_ctl::app_takes(&k, &win),
        "WebView2's own find bar"
    );
    assert!(crate::web_ctl::app_takes(&k, &mac));
    assert!(
        crate::web_ctl::app_takes(&press(0x46, true, true), &win),
        "Ctrl+Shift+F"
    );
    // The new keys of the page's view are no chord on Windows.
    assert!(!crate::web_ctl::app_takes(&press(0xBB, true, false), &win));
    assert!(!crate::web_ctl::app_takes(&press(0xDB, true, true), &win));
}

#[test]
fn the_second_escape_within_600_ms_gives_the_keys_back_and_a_held_escape_counts_once() {
    let (f, slot, id) = mac_page();
    focus_page(&f, &slot, &id);
    let esc = press(0x1B, false, false);
    assert!(!filter(&slot, esc), "the first Escape is the page's");
    let held = KeyPress {
        repeat: true,
        ..esc
    };
    assert!(!filter(&slot, held), "an autorepeat is not a second press");
    assert!(filter(&slot, esc), "the second is the app's");
    handed_back(&slot, &id, esc);
    f.pump();
    assert!(
        !f.ctl.borrow().web.has_focus(),
        "the keys are the terminal's"
    );
    assert!(fake(&slot).calls().contains(&Call::Blur(id.clone())));
    assert!(!f.h.comp.get_panel_web_focused());
    assert_eq!(f.key(KeyEvent::new("a", "KeyA")), KeyOutcome::Pty);
}

#[test]
fn the_focus_chord_moves_the_keys_both_ways_and_a_press_off_the_page_takes_them_back() {
    let (f, slot, id) = mac_page();
    focus_page(&f, &slot, &id);
    // The chord from inside the page (its filter handed it back) gives the terminal the keys.
    handed_back(&slot, &id, press(0x4F, true, true));
    f.pump();
    assert!(!f.ctl.borrow().web.has_focus());
    assert!(!f.ctl.borrow().page_has_keys());
    assert_eq!(f.key(KeyEvent::new("a", "KeyA")), KeyOutcome::Pty);
    focus_page(&f, &slot, &id);
    // A press with the pointer off the page (the terminal, the tab strip) is a press away from it.
    assert!(!f.h.comp.get_panel_web_hovered());
    f.ctl.borrow_mut().on_pointer_down();
    assert!(!f.ctl.borrow().web.has_focus());
    assert!(!f.ctl.borrow().page_has_keys());
}

#[test]
fn the_back_and_forward_buttons_step_through_the_history_and_are_never_a_left_click() {
    let (f, slot, id) = mac_page();
    let downs = |slot: &Slot| {
        fake(slot)
            .calls()
            .into_iter()
            .filter(|c| {
                matches!(c, Call::Pointer(_, p) if matches!(p.act, flare_web::PointerAct::Down(..)))
            })
            .count()
    };
    // Nothing to go back to: the button does nothing.
    f.h.comp.invoke_panel_web_pointer(0, 10.0, 10.0, 3, 0);
    assert!(
        !fake(&slot)
            .calls()
            .contains(&Call::Go(id.clone(), Nav::Back))
    );
    fake(&slot).say(WebEvent::History {
        tab: id.clone(),
        can_back: true,
        can_forward: true,
    });
    f.pump();
    f.h.comp.invoke_panel_web_pointer(0, 10.0, 10.0, 3, 0);
    f.h.comp.invoke_panel_web_pointer(1, 10.0, 10.0, 3, 0);
    f.h.comp.invoke_panel_web_pointer(0, 10.0, 10.0, 4, 0);
    f.h.comp.invoke_panel_web_pointer(0, 10.0, 10.0, 5, 0);
    let calls = fake(&slot).calls();
    assert_eq!(
        calls
            .iter()
            .filter(|c| **c == Call::Go(id.clone(), Nav::Back))
            .count(),
        1,
        "one back, on the press: {calls:?}"
    );
    assert!(calls.contains(&Call::Go(id.clone(), Nav::Forward)));
    assert_eq!(
        downs(&slot),
        0,
        "no button press reached the page as a click"
    );
    // A move with any button is a move.
    f.h.comp.invoke_panel_web_pointer(2, 12.0, 12.0, 5, 0);
    assert!(
        fake(&slot)
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Pointer(_, p) if p.act == flare_web::PointerAct::Move))
    );
}

#[test]
fn the_zoom_keys_zoom_the_page_that_has_the_keys_and_keep_them_there() {
    let (f, slot, id) = mac_page();
    // The terminal has the keys and the pointer is elsewhere: the zoom keys are the terminal's.
    f.ctl.borrow_mut().zoom_front_step(1);
    assert!(zooms(&slot, &id).is_empty());
    focus_page(&f, &slot, &id);
    // Cmd+= from inside the page: handed back, the page goes one of Chromium's steps up, the keys stay.
    handed_back(&slot, &id, press(0xBB, true, false));
    f.pump();
    assert_eq!(zooms(&slot, &id), vec![1.1]);
    assert!(f.ctl.borrow().web.has_focus(), "the page keeps the keys");
    assert!(f.ctl.borrow_mut().zoom_front_step(1));
    assert!(f.ctl.borrow_mut().zoom_front_step(-1));
    assert!(f.ctl.borrow_mut().zoom_front_step(0));
    assert_eq!(zooms(&slot, &id), vec![1.1, 1.25, 1.1, 1.0]);
    assert!((f.ctl.borrow().web.page_zoom(&id) - 1.0).abs() < 1e-9);
    // Cmd+wheel over the page zooms it (a whole step's turn), a plain wheel scrolls it.
    f.h.comp
        .invoke_panel_web_wheel(5.0, 5.0, 0.0, 30.0, CMD_BIT);
    assert_eq!(zooms(&slot, &id).len(), 4, "not yet a step");
    f.h.comp
        .invoke_panel_web_wheel(5.0, 5.0, 0.0, 30.0, CMD_BIT);
    assert_eq!(zooms(&slot, &id).last(), Some(&1.1));
    f.h.comp
        .invoke_panel_web_wheel(5.0, 5.0, 0.0, -120.0, CMD_BIT);
    assert_eq!(
        zooms(&slot, &id).last(),
        Some(&0.9),
        "a long turn is two steps at once"
    );
    let wheels = |slot: &Slot| {
        fake(slot)
            .calls()
            .into_iter()
            .filter(|c| matches!(c, Call::Pointer(_, p) if matches!(p.act, flare_web::PointerAct::Wheel { .. })))
            .count()
    };
    assert_eq!(wheels(&slot), 0, "a zooming wheel never scrolls");
    f.h.comp.invoke_panel_web_wheel(5.0, 5.0, 0.0, 30.0, 0);
    assert_eq!(wheels(&slot), 1);
}

#[test]
fn cmd_f_in_the_page_opens_flares_find_bar_which_drives_the_pages_find() {
    let (f, slot, id) = mac_page();
    focus_page(&f, &slot, &id);
    handed_back(&slot, &id, press(0x46, true, false));
    f.pump();
    f.h.advance(0);
    assert!(
        f.h.comp.get_panel_web_find_open(),
        "the bar is open over the page"
    );
    assert!(f.ctl.borrow().web.page_find_open());
    assert!(
        !f.ctl.borrow().web.has_focus(),
        "its field has the keys, not the page"
    );
    assert!(f.h.comp.get_find_input_focused());
    assert_eq!(
        f.key(KeyEvent::new("a", "KeyA")),
        KeyOutcome::Slint,
        "typing goes to the field"
    );
    f.h.comp.set_panel_web_find_query("needle".into());
    f.h.comp.invoke_panel_web_find_edited("needle".into());
    assert!(
        fake(&slot)
            .calls()
            .contains(&Call::Find(id.clone(), "needle".into(), true, false))
    );
    assert_eq!(f.h.comp.get_panel_web_find_readout(), "");
    let steps = |slot: &Slot, forward: bool| {
        fake(slot)
            .calls()
            .iter()
            .filter(|c| **c == Call::Find(id.clone(), "needle".into(), forward, true))
            .count()
    };
    // CEF counts a new search's matches and selects none: one step selects the first, and the readout waits for it.
    fake(&slot).say(WebEvent::Found {
        tab: id.clone(),
        count: 12,
        active: 0,
    });
    f.pump();
    assert_eq!(steps(&slot, true), 1);
    assert_eq!(f.h.comp.get_panel_web_find_readout(), "");
    fake(&slot).say(WebEvent::Found {
        tab: id.clone(),
        count: 12,
        active: 1,
    });
    f.pump();
    assert_eq!(f.h.comp.get_panel_web_find_readout(), "1 of 12");
    assert!(f.h.comp.get_panel_web_find_can_step());
    f.h.comp.invoke_panel_web_find_next();
    f.h.comp.invoke_panel_web_find_previous();
    assert_eq!(steps(&slot, true), 2, "Enter: the next match");
    assert_eq!(steps(&slot, false), 1, "Shift+Enter: the one before");
    fake(&slot).say(WebEvent::Found {
        tab: id.clone(),
        count: 12,
        active: 3,
    });
    f.pump();
    assert_eq!(f.h.comp.get_panel_web_find_readout(), "3 of 12");
    fake(&slot).say(WebEvent::Found {
        tab: id.clone(),
        count: 0,
        active: 0,
    });
    f.pump();
    assert_eq!(f.h.comp.get_panel_web_find_readout(), "none");
    assert!(!f.h.comp.get_panel_web_find_can_step());
    // Close (its button or Escape): the matches go and the page has the keys again.
    let focus_before = fake(&slot)
        .calls()
        .iter()
        .filter(|c| **c == Call::Focus(id.clone()))
        .count();
    f.h.comp.invoke_panel_web_find_close();
    f.h.advance(0);
    assert!(!f.h.comp.get_panel_web_find_open());
    let calls = fake(&slot).calls();
    assert!(calls.contains(&Call::StopFind(id.clone())));
    assert_eq!(
        calls
            .iter()
            .filter(|c| **c == Call::Focus(id.clone()))
            .count(),
        focus_before + 1
    );
    assert!(f.h.comp.get_panel_web_focused());
}

#[test]
fn the_find_bar_closes_when_its_page_goes_elsewhere() {
    let (f, slot, id) = mac_page();
    focus_page(&f, &slot, &id);
    handed_back(&slot, &id, press(0x46, true, false));
    f.pump();
    assert!(f.h.comp.get_panel_web_find_open());
    load(&fake(&slot), &id, TWO, 2);
    f.pump();
    f.h.advance(0);
    assert!(!f.h.comp.get_panel_web_find_open());
    assert!(fake(&slot).calls().contains(&Call::StopFind(id.clone())));
}

#[test]
fn the_pages_status_text_shows_under_it_and_the_input_method_goes_to_it() {
    let (f, slot, id) = mac_page();
    fake(&slot).say(WebEvent::Status {
        tab: id.clone(),
        text: "https://flare-test.local/two.html".into(),
    });
    f.pump();
    assert_eq!(
        f.h.comp.get_panel_web_status(),
        "https://flare-test.local/two.html"
    );
    fake(&slot).say(WebEvent::Status {
        tab: id.clone(),
        text: String::new(),
    });
    f.pump();
    assert_eq!(f.h.comp.get_panel_web_status(), "");
    // The input method's text is the shell's while the terminal has the keys, the page's once it has them.
    assert!(!f.ctl.borrow_mut().page_ime("ni", Some(2), false));
    focus_page(&f, &slot, &id);
    assert!(f.ctl.borrow_mut().page_ime("你", Some(3), false));
    assert!(f.ctl.borrow_mut().page_ime("", None, false));
    assert!(f.ctl.borrow_mut().page_ime("你好", None, true));
    let imes: Vec<flare_web::ImeInput> = fake(&slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Ime(_, i) => Some(i),
            _ => None,
        })
        .collect();
    assert_eq!(
        imes,
        vec![
            flare_web::ImeInput::Preedit {
                text: "你".into(),
                cursor: 1
            },
            flare_web::ImeInput::Cancel,
            flare_web::ImeInput::Commit("你好".into()),
        ]
    );
}
