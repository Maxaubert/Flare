//! The web panel inside the window controller (Surfaces stage 3), with a stand-in for the browser:
//! when a controller exists, where it is put, what hides it, what the channel's two waiting calls
//! answer and when, the address field, the keys the page hands back and the missing-runtime path.

use std::sync::mpsc::{Receiver, TryRecvError};

use flare_ctl::code;
use flare_surfaces::web::{PhysRect, phys_rect};
use flare_web::fake::{Call, FakeWeb};
use flare_web::{KeyPress, Nav, WebBackend, WebEvent};
use serde_json::Value;

use super::*;
use crate::ctl_app::{CtlOp, NavTo};
use crate::web_ctl::{WebMsg, WebMsgBox};

const PAGE: &str = "https://flare-test.local/index.html";
const TWO: &str = "https://flare-test.local/two.html";

type Slot = Rc<RefCell<Option<FakeWeb>>>;

/// Hands the controller a fake browser; the slot holds it once the first web panel makes it.
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

/// Sends a channel op; the answer comes later through the returned receiver when the call waits.
fn ask(f: &Fx, op: CtlOp, wait: Duration) -> Receiver<Result<Value, flare_ctl::CtlError>> {
    let (tx, rx) = std::sync::mpsc::channel();
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Ctl(crate::ctl_app::CtlRequestBox(
            crate::ctl_app::CtlRequest {
                op,
                deadline: Instant::now() + wait,
                reply: tx,
            },
        )));
    rx
}

fn open_op(tab: &str, url: &str) -> CtlOp {
    CtlOp::BrowserOpen {
        tab: tab.into(),
        url: url.into(),
    }
}

/// The browser starts a navigation and finishes it (the number is the browser's own, one up each time).
fn finish(fake: &FakeWeb, tab: &str, url: &str, ok: bool) {
    thread_local! {
        static NAV: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
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
        ok,
        status: "0".into(),
        nav,
    });
}

/// The browser window's rectangle as Slint lays it out, in device pixels.
fn drawn(f: &Fx) -> PhysRect {
    let a = &f.h.comp;
    phys_rect(
        a.get_panel_web_x(),
        a.get_panel_web_y(),
        a.get_panel_web_w(),
        a.get_panel_web_h(),
        a.get_scale(),
    )
}

fn opened_calls(slot: &Slot) -> Vec<Call> {
    fake(slot)
        .calls()
        .into_iter()
        .filter(|c| matches!(c, Call::Open { .. }))
        .collect()
}

/// The last visibility the fake was told for the tab.
fn last_visible(slot: &Slot, tab: &str) -> Option<bool> {
    fake(slot).calls().into_iter().rev().find_map(|c| match c {
        Call::Visible(t, v) if t == tab => Some(v),
        _ => None,
    })
}

/// A tab in front with the browser up and the page loaded.
fn up(f: &Fx, slot: &Slot) -> String {
    let id = f.ctl.borrow().front_id().unwrap();
    let rx = ask(f, open_op(&id, PAGE), Duration::from_secs(5));
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)), "waits");
    fake(slot).say(WebEvent::Title {
        tab: id.clone(),
        title: "Flare test page".into(),
    });
    finish(&fake(slot), &id, PAGE, true);
    f.pump();
    assert!(rx.try_recv().unwrap().is_ok());
    id
}

#[test]
fn nothing_exists_before_a_web_panel_opens() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    f.h.advance(0);
    assert!(slot.borrow().is_none(), "the browser is not even made");
    assert_eq!(f.ctl.borrow().web_controllers(), 0);
    assert_eq!(f.h.comp.get_panel_kind(), 0);
}

#[test]
fn a_web_panel_makes_its_controller_at_the_slot_and_frees_it_when_closed() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    let opened = opened_calls(&slot);
    assert_eq!(opened.len(), 1);
    let Call::Open {
        tab,
        url,
        bounds,
        visible,
    } = &opened[0]
    else {
        unreachable!()
    };
    assert_eq!((tab.as_str(), url.as_str()), (id.as_str(), PAGE));
    assert_eq!(
        *bounds,
        drawn(&f),
        "the controller is put where Slint drew the box"
    );
    assert!(!bounds.is_empty());
    assert!(!*visible, "it is shown by the next sync, not at birth");
    f.pump();
    assert_eq!(f.ctl.borrow().web_controllers(), 1);
    assert_eq!(f.h.comp.get_panel_kind(), 3);
    assert_eq!(f.h.comp.get_panel_title().as_str(), "Flare test page");
    // The panel closes: the controller and the browser go.
    f.h.comp.invoke_panel_close();
    f.h.advance(0);
    assert_eq!(f.ctl.borrow().web_controllers(), 0);
    assert!(
        fake(&slot).calls().contains(&Call::Close(id)),
        "the controller was closed"
    );
    assert!(
        !f.ctl.borrow().web.has_backend(),
        "the browser object is let go"
    );
}

#[test]
fn the_toggle_chord_brings_the_page_back_at_its_last_address() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    fake(&slot).say(WebEvent::Url {
        tab: id.clone(),
        url: TWO.into(),
    });
    f.pump();
    assert_eq!(f.key(shift_ctrl("P", "KeyP")), KeyOutcome::Consumed);
    f.h.advance(0);
    assert_eq!(f.ctl.borrow().web_controllers(), 0);
    assert_eq!(f.key(shift_ctrl("P", "KeyP")), KeyOutcome::Consumed);
    f.h.advance(0);
    f.pump();
    let again = opened_calls(&slot);
    let Some(Call::Open { url, .. }) = again.last() else {
        panic!("no second open: {again:?}")
    };
    assert_eq!(url, TWO);
    assert_eq!(f.ctl.borrow().web_controllers(), 1);
}

#[test]
fn the_controller_follows_the_slot_when_the_splitter_moves() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    let before = drawn(&f);
    f.ctl.borrow_mut().panel_splitter_released(0.55);
    f.h.advance(0);
    f.pump();
    let after = drawn(&f);
    assert!(after.w > before.w, "{before:?} {after:?}");
    let sent = fake(&slot)
        .calls()
        .into_iter()
        .rev()
        .find_map(|c| match c {
            Call::Bounds(t, r) if t == id => Some(r),
            _ => None,
        })
        .expect("the new bounds were sent");
    assert_eq!(sent, after);
}

#[test]
fn it_sits_on_the_slot_at_every_display_scale() {
    for scale in [1.0f32, 1.5, 2.25] {
        let (f, _h, _) = live(Opts {
            scale,
            ..Opts::default()
        });
        let slot = with_fake(&f);
        let id = up(&f, &slot);
        f.h.advance(0);
        let d = drawn(&f);
        let view = fake(&slot).probe(&id).unwrap();
        assert_eq!(view.bounds, d, "scale {scale}");
        // Device pixels: the page ends on the window's right edge.
        assert_eq!(d.right(), (1000.0 * scale).round() as i32, "scale {scale}");
    }
}

#[test]
fn an_overlay_takes_a_still_and_then_hides_the_window() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    assert_eq!(
        last_visible(&slot, &id),
        Some(true),
        "shown once it had a place"
    );
    // A tiny real PNG for the still.
    let mut bytes = Vec::new();
    {
        let mut e = png::Encoder::new(&mut bytes, 4, 3);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let mut w = e.write_header().unwrap();
        w.write_image_data(&[200u8; 4 * 3 * 4]).unwrap();
    }
    fake(&slot).state.borrow_mut().still = Some(bytes);
    f.h.comp.set_dialog_open(true);
    f.h.advance(0);
    assert!(f.h.comp.get_overlay_open());
    f.pump_until("the still", |f| f.h.comp.get_panel_web_has_still());
    let calls = fake(&slot).calls();
    let capture = calls
        .iter()
        .position(|c| matches!(c, Call::Capture(t, _) if *t == id))
        .expect("a still was asked for");
    let hide = calls
        .iter()
        .position(|c| *c == Call::Visible(id.clone(), false))
        .expect("the window was hidden");
    assert!(
        capture < hide,
        "the still is taken before the window is hidden"
    );
    assert_eq!(f.h.comp.get_panel_web_still().size().width, 4);
    // The overlay closes: the window shows again and the still is let go.
    f.h.comp.set_dialog_open(false);
    f.h.advance(0);
    f.pump();
    assert!(!f.h.comp.get_panel_web_has_still());
    assert_eq!(f.h.comp.get_panel_web_still().size().width, 0);
    assert_eq!(last_visible(&slot, &id), Some(true));
}

#[test]
fn a_still_that_cannot_be_taken_still_hides_the_window() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    f.h.comp.set_menu_open(true);
    f.h.advance(0);
    f.pump();
    assert_eq!(last_visible(&slot, &id), Some(false));
    assert!(!f.h.comp.get_panel_web_has_still());
}

#[test]
fn a_late_still_after_the_overlay_closed_is_dropped() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    // The overlay opens and closes before the browser answers.
    f.h.comp.set_dialog_open(true);
    f.h.advance(0);
    f.h.comp.set_dialog_open(false);
    f.h.advance(0);
    fake(&slot).say(WebEvent::Still {
        tab: id.clone(),
        token: 1,
        png: Some(vec![1, 2, 3]),
    });
    f.pump();
    assert!(!f.h.comp.get_panel_web_has_still());
    assert_eq!(last_visible(&slot, &id), Some(true), "the window stays up");
}

#[test]
fn a_tab_behind_keeps_its_page_but_hides_the_window() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let a = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    let _b = f.ctl.borrow_mut().open_folder("C:\\b");
    f.ctl.borrow_mut().refresh();
    f.h.advance(0);
    f.pump();
    assert_eq!(f.ctl.borrow().web_controllers(), 1, "the page stays alive");
    assert_eq!(last_visible(&slot, &a), Some(false));
    assert_eq!(f.h.comp.get_panel_kind(), 0, "the new tab has no panel");
    // Back on the first tab the window shows again.
    assert_eq!(
        f.key(KeyEvent::new("1", "Digit1").ctrl()),
        KeyOutcome::Consumed
    );
    f.h.advance(0);
    f.pump();
    assert_eq!(last_visible(&slot, &a), Some(true));
    assert_eq!(f.h.comp.get_panel_kind(), 3);
}

#[test]
fn closing_a_tab_frees_its_page() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let a = up(&f, &slot);
    let _b = f.ctl.borrow_mut().open_folder("C:\\b");
    f.ctl.borrow_mut().request_close(&a);
    f.pump();
    f.h.advance(0);
    assert_eq!(f.ctl.borrow().web_controllers(), 0);
    assert!(fake(&slot).calls().contains(&Call::Close(a)));
}

// ---- the two calls that wait ----

#[test]
fn browser_open_answers_when_the_page_has_loaded_and_not_before() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let rx = ask(&f, open_op(&id, PAGE), Duration::from_secs(5));
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    fake(&slot).say(WebEvent::Title {
        tab: id.clone(),
        title: "Flare test page".into(),
    });
    f.pump();
    assert!(
        matches!(rx.try_recv(), Err(TryRecvError::Empty)),
        "a title is not a load"
    );
    finish(&fake(&slot), &id, PAGE, true);
    f.pump();
    let v = rx.try_recv().unwrap().unwrap();
    assert_eq!(v["url"], PAGE);
    assert_eq!(v["title"], "Flare test page");
    assert_eq!(v["loaded"], true);
    assert_eq!(v["panel"]["kind"], "web");
    assert_eq!(v["panel"]["url"], PAGE);
    assert_eq!(v["panel"]["title"], "Flare test page");
    assert!(v["panel"].get("path").is_none());
}

#[test]
fn a_page_with_no_title_is_named_by_its_host_and_a_new_title_is_published() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let events = recorded_events(&f);
    let rx = ask(&f, open_op(&id, PAGE), Duration::from_secs(5));
    finish(&fake(&slot), &id, PAGE, true);
    f.pump();
    rx.try_recv().unwrap().unwrap();
    assert_eq!(
        f.ctl.borrow().panels.snap(&id).unwrap().title,
        "flare-test.local"
    );
    events.lock().unwrap().clear();
    fake(&slot).say(WebEvent::Title {
        tab: id.clone(),
        title: "Docs
home"
            .into(),
    });
    f.pump();
    assert_eq!(f.ctl.borrow().panels.snap(&id).unwrap().title, "Docs home");
    let seen = events.lock().unwrap();
    assert!(
        seen.iter()
            .any(|(name, p, _)| name == "panel.changed" && p["panel"]["title"] == "Docs home"),
        "{seen:?}"
    );
}

#[test]
fn a_load_that_finishes_before_the_calls_own_navigation_is_not_its_answer() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    // An earlier navigation is still on its way when the call is made.
    fake(&slot).say(WebEvent::Loading {
        tab: id.clone(),
        url: TWO.into(),
        nav: 70,
    });
    f.pump();
    let rx = ask(
        &f,
        CtlOp::BrowserNavigate {
            tab: id.clone(),
            to: NavTo::Url(PAGE.into()),
        },
        Duration::from_secs(5),
    );
    // The old one finishes first: not the answer.
    fake(&slot).say(WebEvent::Loaded {
        tab: id.clone(),
        url: TWO.into(),
        ok: true,
        status: "0".into(),
        nav: 70,
    });
    f.pump();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    // The call's own navigation begins and ends: that is.
    fake(&slot).say(WebEvent::Loading {
        tab: id.clone(),
        url: PAGE.into(),
        nav: 71,
    });
    fake(&slot).say(WebEvent::Loaded {
        tab: id,
        url: PAGE.into(),
        ok: true,
        status: "0".into(),
        nav: 71,
    });
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap()["url"], PAGE);
}

#[test]
fn a_redirect_rebinds_the_wait_to_the_last_navigation() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let rx = ask(&f, open_op(&id, PAGE), Duration::from_secs(5));
    for (nav, url) in [(1u64, PAGE), (2, TWO)] {
        fake(&slot).say(WebEvent::Loading {
            tab: id.clone(),
            url: url.into(),
            nav,
        });
    }
    f.pump();
    fake(&slot).say(WebEvent::Loaded {
        tab: id.clone(),
        url: PAGE.into(),
        ok: true,
        status: "0".into(),
        nav: 1,
    });
    f.pump();
    assert!(
        matches!(rx.try_recv(), Err(TryRecvError::Empty)),
        "the redirected-from navigation does not answer"
    );
    fake(&slot).say(WebEvent::Loaded {
        tab: id,
        url: TWO.into(),
        ok: true,
        status: "0".into(),
        nav: 2,
    });
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap()["url"], TWO);
}

#[test]
fn a_refused_navigation_answers_a_waiting_call() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let rx = ask(&f, open_op(&id, PAGE), Duration::from_secs(5));
    fake(&slot).say(WebEvent::Refused {
        tab: id,
        url: "file:///C:/x".into(),
    });
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::UNAVAILABLE);
}

#[test]
fn a_page_that_fails_to_load_still_answers_with_loaded_false() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let rx = ask(&f, open_op(&id, PAGE), Duration::from_secs(5));
    finish(&fake(&slot), &id, PAGE, false);
    f.pump();
    let v = rx.try_recv().unwrap().unwrap();
    assert_eq!(v["loaded"], false);
}

#[test]
fn an_answer_after_the_callers_deadline_is_not_sent() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let rx = ask(&f, open_op(&id, PAGE), Duration::from_millis(30));
    // sync-ok: test code only, lets a 30 ms deadline pass
    std::thread::sleep(Duration::from_millis(60));
    finish(&fake(&slot), &id, PAGE, true);
    f.pump();
    assert!(
        !matches!(rx.try_recv(), Ok(Ok(_))),
        "the caller was told timeout; nothing arrives after"
    );
}

#[test]
fn a_second_call_replaces_the_first_and_the_first_is_told() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let first = ask(&f, open_op(&id, PAGE), Duration::from_secs(5));
    let second = ask(
        &f,
        CtlOp::BrowserNavigate {
            tab: id.clone(),
            to: NavTo::Url(TWO.into()),
        },
        Duration::from_secs(5),
    );
    let e = first.try_recv().unwrap().unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    finish(&fake(&slot), &id, TWO, true);
    f.pump();
    assert_eq!(second.try_recv().unwrap().unwrap()["loaded"], true);
}

#[test]
fn navigate_sends_the_page_on_and_back_needs_history() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let rx = ask(
        &f,
        CtlOp::BrowserNavigate {
            tab: id.clone(),
            to: NavTo::Url(TWO.into()),
        },
        Duration::from_secs(5),
    );
    assert!(
        fake(&slot)
            .calls()
            .contains(&Call::Navigate(id.clone(), TWO.into()))
    );
    finish(&fake(&slot), &id, TWO, true);
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap()["url"], TWO);
    // No earlier page known yet.
    let r = ask(
        &f,
        CtlOp::BrowserNavigate {
            tab: id.clone(),
            to: NavTo::Back,
        },
        Duration::from_secs(5),
    );
    assert_eq!(r.try_recv().unwrap().unwrap_err().code, code::UNAVAILABLE);
    fake(&slot).say(WebEvent::History {
        tab: id.clone(),
        can_back: true,
        can_forward: false,
    });
    f.pump();
    let r = ask(
        &f,
        CtlOp::BrowserNavigate {
            tab: id.clone(),
            to: NavTo::Back,
        },
        Duration::from_secs(5),
    );
    assert!(
        fake(&slot)
            .calls()
            .contains(&Call::Go(id.clone(), Nav::Back))
    );
    finish(&fake(&slot), &id, PAGE, true);
    f.pump();
    assert_eq!(r.try_recv().unwrap().unwrap()["url"], PAGE);
    // The panel points at the page it is on.
    assert_eq!(f.ctl.borrow().panels.web_target(&id).as_deref(), Some(PAGE));
}

#[test]
fn navigate_with_no_web_panel_is_unavailable() {
    let (f, _h, _) = live(Opts::default());
    with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let r = ask(
        &f,
        CtlOp::BrowserNavigate {
            tab: id,
            to: NavTo::Reload,
        },
        Duration::from_secs(5),
    );
    assert_eq!(r.try_recv().unwrap().unwrap_err().code, code::UNAVAILABLE);
}

#[test]
fn a_tab_behind_answers_at_once_with_what_is_stored() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let a = f.ctl.borrow().front_id().unwrap();
    let _b = f.ctl.borrow_mut().open_folder("C:\\b");
    f.ctl.borrow_mut().refresh();
    // The first tab is behind: its panel is stored, no browser is made for it, and the call does not
    // wait for a load that cannot happen.
    let rx = ask(&f, open_op(&a, PAGE), Duration::from_secs(5));
    let v = rx.try_recv().unwrap().unwrap();
    assert_eq!(v["url"], PAGE);
    assert_eq!(v["loaded"], false);
    assert!(slot.borrow().is_none(), "no browser for a tab nobody sees");
    assert_eq!(f.ctl.borrow().web_controllers(), 0);
    // It is made when the tab comes to the front.
    assert_eq!(
        f.key(KeyEvent::new("1", "Digit1").ctrl()),
        KeyOutcome::Consumed
    );
    f.h.advance(0);
    f.pump();
    assert_eq!(f.ctl.borrow().web_controllers(), 1);
    let o = opened_calls(&slot);
    let Some(Call::Open { tab, url, .. }) = o.first() else {
        panic!("{o:?}")
    };
    assert_eq!((tab.as_str(), url.as_str()), (a.as_str(), PAGE));
}

#[test]
fn an_unknown_tab_is_no_tab() {
    let (f, _h, _) = live(Opts::default());
    with_fake(&f);
    let r = ask(&f, open_op("nope", PAGE), Duration::from_secs(5));
    assert_eq!(r.try_recv().unwrap().unwrap_err().code, code::NO_TAB);
}

// ---- the runtime that is not there ----

#[test]
fn a_missing_runtime_is_a_message_with_the_link_and_nothing_else_breaks() {
    let (f, _h, _) = live(Opts::default());
    let id = f.ctl.borrow().front_id().unwrap();
    // The browser is made on the first call; it fails the way a missing runtime does.
    f.ctl.borrow_mut().set_web_factory(Box::new(move |sink| {
        let fake = FakeWeb::new(sink);
        fake.state.borrow_mut().fail = Some(("no runtime".into(), true));
        Box::new(fake)
    }));
    let rx = ask(&f, open_op(&id, PAGE), Duration::from_secs(5));
    f.pump();
    let e = rx.try_recv().unwrap().unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    assert!(e.message.contains("go.microsoft.com"), "{}", e.message);
    f.h.advance(0);
    assert_eq!(f.h.comp.get_panel_kind(), 3);
    assert!(f.h.comp.get_panel_web_missing_runtime());
    assert!(
        f.h.comp
            .get_panel_note()
            .contains(crate::web_ctl::RUNTIME_URL),
        "{}",
        f.h.comp.get_panel_note()
    );
    // The button opens Microsoft's page in the browser the user has.
    f.h.comp.invoke_panel_web_download();
    assert_eq!(
        f.log.borrow().urls.last().map(String::as_str),
        Some(crate::web_ctl::RUNTIME_URL)
    );
    // Nothing else broke: a text panel still opens.
    let text = CtlOp::ShowText {
        tab: id,
        text: "still fine".into(),
        title: None,
    };
    let v = ask(&f, text, Duration::from_secs(5)).try_recv().unwrap();
    assert!(v.is_ok());
    f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
}

#[test]
fn a_build_with_no_browser_at_all_says_so_without_a_crash() {
    let (f, _h, _) = live(Opts::default());
    let id = f.ctl.borrow().front_id().unwrap();
    let rx = ask(&f, open_op(&id, PAGE), Duration::from_secs(5));
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::UNAVAILABLE);
}

// ---- the address field ----

#[test]
fn the_address_field_opens_what_is_typed() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.comp.invoke_panel_web_submit("example.com/docs".into());
    assert!(fake(&slot).calls().contains(&Call::Navigate(
        id.clone(),
        "https://example.com/docs".into()
    )));
    assert_eq!(
        f.ctl.borrow().panels.web_target(&id).as_deref(),
        Some("https://example.com/docs")
    );
    f.h.comp.invoke_panel_web_submit("localhost:3000".into());
    assert!(
        fake(&slot)
            .calls()
            .contains(&Call::Navigate(id, "http://localhost:3000/".into()))
    );
}

#[test]
fn the_address_field_refuses_what_a_panel_may_not_open() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let before = fake(&slot).calls().len();
    for bad in [
        "file:///C:/Windows/win.ini",
        "javascript:alert(1)",
        "two words",
        "",
    ] {
        f.h.comp.invoke_panel_web_submit(bad.into());
        f.h.advance(0);
        assert_eq!(
            fake(&slot).calls().len(),
            before,
            "{bad:?} reached the browser"
        );
    }
    assert_eq!(f.ctl.borrow().panels.web_target(&id).as_deref(), Some(PAGE));
    f.h.comp.invoke_panel_web_submit("file:///C:/x".into());
    f.h.advance(0);
    assert!(
        f.h.comp.get_panel_sub().contains("http"),
        "the header says why: {}",
        f.h.comp.get_panel_sub()
    );
}

#[test]
fn the_bar_buttons_drive_the_page_only_when_they_can() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.comp.invoke_panel_web_back();
    f.h.comp.invoke_panel_web_forward();
    assert!(
        !fake(&slot)
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Go(_, Nav::Back | Nav::Forward)))
    );
    fake(&slot).say(WebEvent::History {
        tab: id.clone(),
        can_back: true,
        can_forward: true,
    });
    f.pump();
    f.h.advance(0);
    assert!(f.h.comp.get_panel_web_can_back() && f.h.comp.get_panel_web_can_forward());
    f.h.comp.invoke_panel_web_back();
    f.h.comp.invoke_panel_web_forward();
    f.h.comp.invoke_panel_web_reload();
    f.h.comp.invoke_panel_web_stop();
    let calls = fake(&slot).calls();
    for nav in [Nav::Back, Nav::Forward, Nav::Reload, Nav::Stop] {
        assert!(calls.contains(&Call::Go(id.clone(), nav)), "{nav:?}");
    }
}

#[test]
fn the_bar_follows_the_page() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    fake(&slot).say(WebEvent::Loading {
        tab: id.clone(),
        url: TWO.into(),
        nav: 900,
    });
    f.pump();
    f.h.advance(0);
    assert!(f.h.comp.get_panel_web_loading());
    fake(&slot).say(WebEvent::Title {
        tab: id.clone(),
        title: "Page two".into(),
    });
    fake(&slot).say(WebEvent::Loaded {
        tab: id.clone(),
        url: TWO.into(),
        ok: true,
        status: "0".into(),
        nav: 900,
    });
    f.pump();
    f.h.advance(0);
    assert!(!f.h.comp.get_panel_web_loading());
    assert_eq!(f.h.comp.get_panel_title().as_str(), "Page two");
    assert_eq!(f.h.comp.get_panel_web_url().as_str(), TWO);
}

#[test]
fn a_refused_navigation_is_said_in_the_header_and_leaves_the_page() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    fake(&slot).say(WebEvent::Refused {
        tab: id.clone(),
        url: "file:///C:/Windows/win.ini".into(),
    });
    f.pump();
    f.h.advance(0);
    assert!(f.h.comp.get_panel_sub().starts_with("Not opened here"));
    assert_eq!(f.ctl.borrow().panels.web_target(&id).as_deref(), Some(PAGE));
}

// ---- keys ----

fn page_key(tab: &str, vk: u32, ctrl: bool, shift: bool) -> WebEvent {
    WebEvent::Key {
        tab: tab.into(),
        key: KeyPress {
            vk,
            ctrl,
            shift,
            alt: false,
            down: true,
            repeat: false,
        },
    }
}

#[test]
fn a_chord_the_page_hands_back_is_the_apps() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    // Ctrl+Shift+P from inside the page closes the panel.
    fake(&slot).say(page_key(&id, 0x50, true, true));
    f.pump();
    f.h.advance(0);
    assert_eq!(f.ctl.borrow().web_controllers(), 0);
}

#[test]
fn the_second_escape_gives_the_keys_back_to_the_terminal() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    fake(&slot).say(WebEvent::Focus {
        tab: id.clone(),
        has: true,
    });
    f.pump();
    assert!(f.ctl.borrow().web.has_focus());
    fake(&slot).say(page_key(&id, 0x1B, false, false));
    f.pump();
    assert!(
        !f.ctl.borrow().web.has_focus(),
        "the keys are the terminal's"
    );
}

#[test]
fn the_focus_chord_hands_the_keys_to_the_page_and_back() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    assert_eq!(f.key(shift_ctrl("O", "KeyO")), KeyOutcome::Consumed);
    assert!(fake(&slot).calls().contains(&Call::Focus(id.clone())));
    fake(&slot).say(WebEvent::Focus {
        tab: id.clone(),
        has: true,
    });
    f.pump();
    assert_eq!(f.key(shift_ctrl("O", "KeyO")), KeyOutcome::Consumed);
    assert!(!f.ctl.borrow().web.has_focus());
}

#[test]
fn a_press_on_the_window_takes_the_keys_back_from_the_page() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    fake(&slot).say(WebEvent::Focus { tab: id, has: true });
    f.pump();
    assert!(f.ctl.borrow().web.has_focus());
    f.ctl.borrow_mut().on_pointer_down();
    assert!(!f.ctl.borrow().web.has_focus());
}

#[test]
fn a_stored_web_address_comes_back_and_a_bad_stored_address_is_dropped() {
    let f = fx_with(Opts {
        saved: vec![("C:\\a", None), ("C:\\b", None)],
        ..Opts::default()
    });
    let file = f.dir.path().join("panels.json");
    let raw = r#"{"version":1,"tabs":[
        {"cwd":"C:\\a","panel":{"kind":"web","target":"https://flare-test.local/two.html"}},
        {"cwd":"C:\\b","panel":{"kind":"web","target":"file:///C:/Windows/win.ini"}}]}"#;
    std::fs::write(&file, raw).unwrap(); // sync-ok: test code only
    f.ctl
        .borrow_mut()
        .set_panel_store(Arc::new(flare_surfaces::store::PanelStore::new(
            &file,
            Duration::from_millis(30),
        )));
    let slot = with_fake(&f);
    f.start_restore_and_wait();
    f.h.advance(0);
    f.pump();
    assert_eq!(
        f.ctl.borrow().open_panels(),
        1,
        "only the web address survives"
    );
    let o = opened_calls(&slot);
    let Some(Call::Open { url, .. }) = o.first() else {
        panic!("{o:?}")
    };
    assert_eq!(url, TWO);
}

#[test]
fn a_decoded_still_for_a_tab_no_longer_shown_changes_nothing() {
    let (f, _h, _) = live(Opts::default());
    with_fake(&f);
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Web(WebMsgBox(WebMsg::Decoded {
            tab: "gone".into(),
            token: 9,
            image: None,
        })));
    assert!(!f.h.comp.get_panel_web_has_still());
}

#[test]
fn the_panel_reports_the_url_and_no_path() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let s = f.ctl.borrow().panels.snap(&id).unwrap();
    assert_eq!(s.kind, "web");
    assert_eq!(s.url.as_deref(), Some(PAGE));
    assert!(s.path.is_none());
    assert_eq!(
        s.title, "Flare test page",
        "named by the page, as the header is"
    );
    let v = ask(&f, CtlOp::PanelState { tab: id }, Duration::from_secs(5))
        .try_recv()
        .unwrap()
        .unwrap();
    assert_eq!(v["open"], true);
    assert_eq!(v["panel"]["kind"], "web");
    assert_eq!(v["panel"]["url"], PAGE);
    assert!(v["panel"].get("path").is_none());
}

// ---- review fixes: the end of life, a page that hangs, focus, failures, downloads ----

fn index_of(calls: &[Call], want: &Call) -> usize {
    calls
        .iter()
        .position(|c| c == want)
        .unwrap_or_else(|| panic!("{want:?} not in {calls:?}"))
}

#[test]
fn quitting_closes_every_web_view_before_the_window_goes() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let a = up(&f, &slot);
    // A call still waiting for its page is told, not left hanging.
    let rx = ask(
        &f,
        CtlOp::BrowserNavigate {
            tab: a.clone(),
            to: NavTo::Url(TWO.into()),
        },
        Duration::from_secs(5),
    );
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    assert_eq!(
        f.ctl.borrow_mut().on_close_requested(),
        CloseDecision::Close
    );
    let calls = fake(&slot).calls();
    assert!(calls.contains(&Call::Close(a)), "{calls:?}");
    assert!(calls.contains(&Call::CloseAll));
    assert_eq!(f.ctl.borrow().web_controllers(), 0);
    let err = rx.try_recv().expect("answered").unwrap_err();
    assert_eq!(err.code, code::UNAVAILABLE);
    // Nothing opens a view again once the window is ending.
    f.h.advance(0);
    f.pump();
    assert_eq!(f.ctl.borrow().web_controllers(), 0);
    assert_eq!(opened_calls(&slot).len(), 1);
}

#[test]
fn a_page_that_never_answers_a_capture_is_hidden_by_the_timeout() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    fake(&slot).state.borrow_mut().still_silent = true;
    f.h.comp.set_dialog_open(true);
    f.h.advance(0);
    f.pump();
    assert_eq!(last_visible(&slot, &id), Some(true), "still waiting");
    assert_eq!(f.ctl.borrow().web.phase_name(), "capturing");
    // The page stays silent past the timeout: the window comes off the screen anyway.
    f.h.advance(flare_surfaces::web::CAPTURE_TIMEOUT_MS + 50);
    f.pump_until("the timeout", |f| {
        f.ctl.borrow().web.phase_name() == "still"
    });
    assert_eq!(last_visible(&slot, &id), Some(false));
    assert!(!f.h.comp.get_panel_web_has_still(), "no picture was taken");
    // The answer of a page that was only slow still fills the place, for the overlay that is still up.
    let token = fake(&slot)
        .calls()
        .into_iter()
        .find_map(|c| match c {
            Call::Capture(_, t) => Some(t),
            _ => None,
        })
        .expect("a capture was asked for");
    fake(&slot).say(WebEvent::Still {
        tab: id.clone(),
        token,
        png: Some(tiny_png()),
    });
    f.pump_until("the late still", |f| f.h.comp.get_panel_web_has_still());
    assert_eq!(f.ctl.borrow().web.phase_name(), "still");
    // Once the overlay closes, nothing late can come back.
    f.h.comp.set_dialog_open(false);
    f.h.advance(0);
    f.pump();
    assert!(!f.h.comp.get_panel_web_has_still());
    fake(&slot).say(WebEvent::Still {
        tab: id,
        token,
        png: Some(tiny_png()),
    });
    f.pump();
    assert!(!f.h.comp.get_panel_web_has_still());
}

fn tiny_png() -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut e = png::Encoder::new(&mut bytes, 4, 3);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    let mut w = e.write_header().unwrap();
    w.write_image_data(&[200u8; 4 * 3 * 4]).unwrap();
    drop(w);
    bytes
}

#[test]
fn closing_the_tab_whose_page_has_the_keys_hands_them_back_first() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let _b = f.ctl.borrow_mut().open_folder("C:\\b");
    let id = up(&f, &slot);
    f.h.advance(0);
    fake(&slot).say(WebEvent::Focus {
        tab: id.clone(),
        has: true,
    });
    f.pump();
    assert!(f.ctl.borrow().web.has_focus());
    // Ctrl+W typed in the page closes its tab.
    fake(&slot).say(page_key(&id, 0x57, true, false));
    f.pump();
    f.h.advance(0);
    assert!(!f.ctl.borrow().web.has_focus(), "the window has the keys");
    let calls = fake(&slot).calls();
    let hide = index_of(&calls, &Call::Visible(id.clone(), false));
    let close = index_of(&calls, &Call::Close(id));
    assert!(hide < close, "the window comes off before it is closed");
}

#[test]
fn a_lost_renderer_takes_the_window_off_and_says_so() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    fake(&slot).say(WebEvent::Focus {
        tab: id.clone(),
        has: true,
    });
    fake(&slot).say(WebEvent::Gone {
        tab: id.clone(),
        why: "the page's process ended".into(),
    });
    f.pump();
    f.h.advance(0);
    assert_eq!(
        last_visible(&slot, &id),
        Some(false),
        "the crash face is not left on top"
    );
    assert!(!f.ctl.borrow().web.has_focus());
    assert!(f.h.comp.get_panel_note().contains("could not be shown"));
    // An overlay closing does not bring the dead page back over the note.
    f.h.comp.set_dialog_open(true);
    f.h.advance(0);
    f.h.comp.set_dialog_open(false);
    f.h.advance(0);
    f.pump();
    assert_eq!(last_visible(&slot, &id), Some(false));
}

#[test]
fn a_lost_browser_marks_the_page_and_closing_the_panel_lets_the_next_start_fresh() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    // The host reports a browser that died as a Gone for each tab and forgets its views.
    fake(&slot).say(WebEvent::Gone {
        tab: id.clone(),
        why: "the browser's process ended".into(),
    });
    f.pump();
    assert!(f.ctl.borrow().web.failure(&id).is_some());
    // Closing the panel frees the tab; the last one takes the browser object with it, so the next
    // open asks the factory again.
    assert_eq!(f.key(shift_ctrl("P", "KeyP")), KeyOutcome::Consumed);
    f.h.advance(0);
    f.pump();
    assert!(!f.ctl.borrow().web.has_backend());
}

#[test]
fn a_navigate_to_a_fragment_of_the_same_page_answers() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let target = format!("{PAGE}#section");
    let rx = ask(
        &f,
        CtlOp::BrowserNavigate {
            tab: id.clone(),
            to: NavTo::Url(target.clone()),
        },
        Duration::from_secs(5),
    );
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    // The browser raises no navigation for it, only a source change inside the document.
    fake(&slot).say(WebEvent::SameDocument {
        tab: id,
        url: target.clone(),
    });
    f.pump();
    let v = rx.try_recv().expect("answered").unwrap();
    assert_eq!(v["url"], target.as_str());
    assert_eq!(v["loaded"], true);
}

#[test]
fn a_stale_load_in_the_queue_does_not_become_the_calls_navigation() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let rx = ask(
        &f,
        CtlOp::BrowserNavigate {
            tab: id.clone(),
            to: NavTo::Url(PAGE.into()),
        },
        Duration::from_secs(5),
    );
    // A Loading for an earlier navigation to another page reaches the window after the call was made.
    fake(&slot).say(WebEvent::Loading {
        tab: id.clone(),
        url: TWO.into(),
        nav: 90,
    });
    fake(&slot).say(WebEvent::Loaded {
        tab: id.clone(),
        url: TWO.into(),
        ok: true,
        status: "0".into(),
        nav: 90,
    });
    f.pump();
    assert!(
        matches!(rx.try_recv(), Err(TryRecvError::Empty)),
        "that load is not the answer"
    );
    finish(&fake(&slot), &id, PAGE, true);
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap()["url"], PAGE);
}

// ---- downloads (S6) ----

fn download(fake: &FakeWeb, tab: &str, id: u64, file: &str) {
    fake.say(WebEvent::Download {
        tab: tab.into(),
        id,
        file: file.into(),
        host: "files.example".into(),
    });
}

fn answers(slot: &Slot) -> Vec<(u64, bool)> {
    fake(slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Download(id, ok) => Some((id, ok)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_download_is_asked_about_and_saved_only_on_a_yes() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    download(&fake(&slot), &id, 1, "report.pdf");
    f.pump();
    assert!(f.h.comp.get_dialog_open());
    assert_eq!(f.h.comp.get_dialog_title().as_str(), "Save this file?");
    assert_eq!(f.h.comp.get_dialog_primary().as_str(), "Save");
    assert!(
        answers(&slot).is_empty(),
        "nothing is saved before the answer"
    );
    f.h.comp.invoke_dialog_confirm();
    assert_eq!(answers(&slot), vec![(1, true)]);
    assert!(!f.h.comp.get_dialog_open());
}

#[test]
fn a_no_cancels_the_download_and_a_second_one_waits_its_turn() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    download(&fake(&slot), &id, 1, "a.zip");
    download(&fake(&slot), &id, 2, "b.zip");
    f.pump();
    assert!(answers(&slot).is_empty());
    f.h.comp.invoke_dialog_cancel();
    assert_eq!(answers(&slot), vec![(1, false)]);
    assert!(f.h.comp.get_dialog_open(), "the second is asked now");
    f.h.comp.invoke_dialog_confirm();
    assert_eq!(answers(&slot), vec![(1, false), (2, true)]);
    assert!(!f.h.comp.get_dialog_open());
}

#[test]
fn a_download_of_a_page_that_is_closed_is_not_asked_about() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    download(&fake(&slot), &id, 1, "a.zip");
    f.pump();
    assert!(f.h.comp.get_dialog_open());
    assert_eq!(f.key(shift_ctrl("P", "KeyP")), KeyOutcome::Consumed);
    f.h.advance(0);
    f.pump();
    assert!(
        !f.h.comp.get_dialog_open(),
        "the question went with the page"
    );
}
