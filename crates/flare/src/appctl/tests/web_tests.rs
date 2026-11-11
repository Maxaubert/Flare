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
fn moving_the_window_tells_the_browser_where_its_parent_is() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let _ = up(&f, &slot);
    f.h.advance(0);
    assert!(!fake(&slot).calls().contains(&Call::ParentMoved));
    let o = observed(1000, 700, 1.0, false);
    f.ctl.borrow_mut().on_geometry(o);
    assert!(
        fake(&slot).calls().contains(&Call::ParentMoved),
        "the select lists and the IME would stay at the old place"
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
        "",
        "   ",
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
        "the address row says why: {}",
        f.h.comp.get_panel_sub()
    );
}

/// 2026-10-28: `youtube` opened `https://youtube/`, a host that does not exist, and two words were refused.
#[test]
fn the_address_field_searches_google_for_what_is_not_an_address() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    for (typed, url) in [
        ("youtube", "https://www.google.com/search?q=youtube"),
        ("cat videos", "https://www.google.com/search?q=cat+videos"),
        ("youtube.com", "https://youtube.com/"),
    ] {
        f.h.comp.invoke_panel_web_submit(typed.into());
        f.h.advance(0);
        let navigated: Vec<Call> = fake(&slot)
            .calls()
            .into_iter()
            .filter(|c| matches!(c, Call::Navigate(..)))
            .collect();
        assert_eq!(
            navigated.last(),
            Some(&Call::Navigate(id.clone(), url.into())),
            "{typed:?}"
        );
        assert_eq!(f.ctl.borrow().panels.web_target(&id).as_deref(), Some(url));
    }
    // An empty field: nothing is sent and nothing is said.
    let before = fake(&slot).calls().len();
    f.h.comp.invoke_panel_web_submit("  ".into());
    f.h.advance(0);
    assert_eq!(fake(&slot).calls().len(), before);
    assert_eq!(f.h.comp.get_panel_sub().as_str(), "");
}

#[test]
fn a_site_that_cannot_be_reached_is_said_in_the_address_row_until_the_next_load() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let say_load = |url: &str, nav: u64, ok: bool, status: &str| {
        fake(&slot).say(WebEvent::Loading {
            tab: id.clone(),
            url: url.into(),
            nav,
        });
        fake(&slot).say(WebEvent::Loaded {
            tab: id.clone(),
            url: url.into(),
            ok,
            status: status.into(),
            nav,
        });
        f.pump();
        f.h.advance(0);
    };
    // The host does not resolve (COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED).
    say_load("https://youtube/", 950, false, "13");
    assert_eq!(f.h.comp.get_panel_sub().as_str(), "Could not open youtube");
    // The next page that loads clears it.
    say_load(TWO, 951, true, "0");
    assert_eq!(f.h.comp.get_panel_sub().as_str(), "");
    // A navigation stopped or replaced, and a stream handed to the player, are not failures to show.
    say_load("https://x.org/", 952, false, "14");
    say_load("https://x.org/a.mp3", 953, false, "9");
    assert_eq!(f.h.comp.get_panel_sub().as_str(), "");
    // An older navigation that fails after a newer one started says nothing about the page now loading.
    fake(&slot).say(WebEvent::Loading {
        tab: id.clone(),
        url: TWO.into(),
        nav: 955,
    });
    fake(&slot).say(WebEvent::Loaded {
        tab: id.clone(),
        url: "https://gone.example/".into(),
        ok: false,
        status: "13".into(),
        nav: 954,
    });
    f.pump();
    f.h.advance(0);
    assert_eq!(f.h.comp.get_panel_sub().as_str(), "");
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

/// The review (2026-10-24): a sound file opened after a page kept that page's title in the header (the browser
/// raises no title change for it, and its load "fails" as the stream goes to the player).
#[test]
fn a_new_document_with_no_title_does_not_wear_the_last_pages() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    fake(&slot).say(WebEvent::Title {
        tab: id.clone(),
        title: "doc.pdf".into(),
    });
    f.pump();
    f.h.advance(0);
    assert_eq!(f.h.comp.get_panel_title().as_str(), "doc.pdf");
    fake(&slot).say(WebEvent::Loading {
        tab: id.clone(),
        url: TWO.into(),
        nav: 901,
    });
    fake(&slot).say(WebEvent::Url {
        tab: id.clone(),
        url: TWO.into(),
    });
    fake(&slot).say(WebEvent::Loaded {
        tab: id.clone(),
        url: TWO.into(),
        ok: false,
        status: "9".into(),
        nav: 901,
    });
    f.pump();
    f.h.advance(0);
    assert_ne!(f.h.comp.get_panel_title().as_str(), "doc.pdf");
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
fn a_chord_that_opens_a_field_of_the_window_takes_the_keyboard_from_the_page() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    fake(&slot).say(WebEvent::Focus {
        tab: id.clone(),
        has: true,
    });
    f.pump();
    assert!(f.ctl.borrow().web.has_focus());
    // Ctrl+Shift+F from inside the page: the find bar opens, and what is typed next is the bar's.
    fake(&slot).say(page_key(&id, 0x46, true, true));
    f.pump();
    f.h.advance(0);
    assert!(
        !f.ctl.borrow().web.has_focus(),
        "the page kept the keyboard through a chord of the window's"
    );
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
    let v = ask(
        &f,
        CtlOp::PanelState { tab: id, cwd: None },
        Duration::from_secs(5),
    )
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
fn a_page_whose_renderer_died_starts_again_for_the_next_open() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    fake(&slot).say(WebEvent::Gone {
        tab: id.clone(),
        why: "the page's process ended".into(),
    });
    f.pump();
    assert!(f.ctl.borrow().web.failure(&id).is_some());
    // The agent opens a page again: it is made anew (not driven hidden behind the note), and answers.
    let rx = ask(&f, open_op(&id, TWO), Duration::from_secs(5));
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)), "waits");
    f.h.advance(0);
    f.pump();
    // (The browser object went with its last view, so the slot holds a new one that was asked to open.)
    let made = opened_calls(&slot);
    assert!(
        matches!(made.as_slice(), [Call::Open { url, .. }] if url == TWO),
        "a new view was made for the address: {made:?}"
    );
    assert!(f.ctl.borrow().web.failure(&id).is_none());
    finish(&fake(&slot), &id, TWO, true);
    f.pump();
    assert!(rx.try_recv().unwrap().is_ok());
    // And a step through the history of a dead page says so instead of driving it.
    fake(&slot).say(WebEvent::Gone {
        tab: id.clone(),
        why: "the page's process ended".into(),
    });
    f.pump();
    let before = fake(&slot).calls().len();
    let rx = ask(
        &f,
        CtlOp::BrowserNavigate {
            tab: id,
            to: NavTo::Reload,
        },
        Duration::from_secs(5),
    );
    f.pump();
    assert!(rx.try_recv().unwrap().is_err());
    assert!(
        !fake(&slot)
            .calls()
            .iter()
            .skip(before)
            .any(|c| matches!(c, Call::Go(..))),
        "nothing navigated the dead page"
    );
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
fn back_over_a_same_document_history_entry_answers_at_once() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    fake(&slot).say(WebEvent::History {
        tab: id.clone(),
        can_back: true,
        can_forward: false,
    });
    f.pump();
    let rx = ask(
        &f,
        CtlOp::BrowserNavigate {
            tab: id.clone(),
            to: NavTo::Back,
        },
        Duration::from_secs(5),
    );
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    // A step inside one document (pushState, a #fragment) raises no navigation, only this.
    fake(&slot).say(WebEvent::SameDocument {
        tab: id,
        url: format!("{PAGE}#before"),
    });
    f.pump();
    let v = rx
        .try_recv()
        .expect("answered, not left to time out")
        .unwrap();
    assert_eq!(v["url"], format!("{PAGE}#before").as_str());
    assert_eq!(v["loaded"], true);
}

#[test]
fn a_pages_own_replace_state_does_not_answer_a_back_that_has_not_happened() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    fake(&slot).say(WebEvent::Loaded {
        tab: id.clone(),
        url: PAGE.into(),
        ok: true,
        status: String::new(),
        nav: 1,
    });
    fake(&slot).say(WebEvent::History {
        tab: id.clone(),
        can_back: true,
        can_forward: false,
    });
    f.pump();
    let rx = ask(
        &f,
        CtlOp::BrowserNavigate {
            tab: id.clone(),
            to: NavTo::Back,
        },
        Duration::from_secs(5),
    );
    // The page rewrites its own address (same url) before the history step: not the call's answer.
    fake(&slot).say(WebEvent::SameDocument {
        tab: id,
        url: PAGE.into(),
    });
    f.pump();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
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

/// The review (2026-10-24): a download started by a click in the page left the keys in the page, so Enter and
/// Escape went to it, not to the question.
#[test]
fn the_download_question_takes_the_keys_back_from_the_page() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    fake(&slot).say(WebEvent::Focus {
        tab: id.clone(),
        has: true,
    });
    f.pump();
    assert!(f.ctl.borrow().web.has_focus());
    download(&fake(&slot), &id, 1, "report.pdf");
    f.pump();
    assert!(f.h.comp.get_dialog_open());
    assert!(!f.ctl.borrow().web.has_focus(), "the question has the keys");
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

// ---- the agent uses the page (stage 4) ----

use flare_surfaces::agent_job::Op as AgentOp;
use flare_surfaces::agent_web::{Button, WaitFor};
use serde_json::json;

fn agent_op(tab: &str, op: AgentOp) -> CtlOp {
    CtlOp::BrowserAgent {
        tab: tab.into(),
        op,
        cancel: flare_ctl::CancelFlag::default(),
    }
}

fn cdp_calls(slot: &Slot) -> Vec<(String, String)> {
    fake(slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Cdp(_, _, m, p) => Some((m, p)),
            _ => None,
        })
        .collect()
}

/// A page with a button and a field, answering the DevTools calls a look makes.
fn shop(method: &str, params: &str) -> Result<String, String> {
    let p: Value = serde_json::from_str(params).unwrap_or(Value::Null);
    let quad = |x: f64, y: f64, w: f64, h: f64| {
        json!({"quads": [[x, y, x + w, y, x + w, y + h, x, y + h]]}).to_string()
    };
    match method {
        "Page.getLayoutMetrics" => Ok(json!({"cssVisualViewport": {"pageX": 0, "pageY": 0, "clientWidth": 600, "clientHeight": 400}}).to_string()),
        "Accessibility.getFullAXTree" => Ok(json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "childIds": ["2", "3"], "backendDOMNodeId": 1},
            {"nodeId": "2", "role": {"value": "button"}, "name": {"value": "Buy"}, "backendDOMNodeId": 11},
            {"nodeId": "3", "role": {"value": "textbox"}, "name": {"value": "Quantity"}, "backendDOMNodeId": 12}
        ]}).to_string()),
        "Page.captureScreenshot" => Ok(json!({"data": "iVBORw0KGgoAAAANSUhEUgAAAyAAAAH0CAYAAADL1t+KAAAAAXNSR0IArs4c6QAAAA"}).to_string()),
        "DOM.getContentQuads" => match p["backendNodeId"].as_i64() {
            Some(11) => Ok(quad(100.0, 100.0, 80.0, 20.0)),
            Some(12) => Ok(quad(100.0, 150.0, 200.0, 20.0)),
            _ => Err("no box".into()),
        },
        _ => Ok("{}".into()),
    }
}

fn with_shop(slot: &Slot) {
    fake(slot).state.borrow_mut().cdp = Some(Box::new(shop));
}

fn look(f: &Fx, tab: &str) -> Receiver<Result<Value, flare_ctl::CtlError>> {
    ask(
        f,
        agent_op(tab, AgentOp::Look { picture: true }),
        Duration::from_secs(5),
    )
}

/// The agent's clock moves on `ms` and its timer turns once (a glide, the hold, a fade pass).
fn agent_turn(f: &Fx, ms: u64) {
    f.ctl.borrow_mut().web.skip_clock(ms);
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Web(WebMsgBox(WebMsg::AgentTick)));
    f.pump();
}

fn click_ref(f: &Fx, tab: &str, r: &str) -> Receiver<Result<Value, flare_ctl::CtlError>> {
    ask(
        f,
        agent_op(
            tab,
            AgentOp::Click {
                target: flare_surfaces::agent_job::Target::Ref(r.into()),
                button: Button::Left,
                double: false,
            },
        ),
        Duration::from_secs(5),
    )
}

#[test]
fn a_look_goes_through_devtools_and_answers_with_the_picture_and_the_elements() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    with_shop(&slot);
    let rx = look(&f, &id);
    f.pump();
    let v = rx.try_recv().expect("answered").expect("ok");
    assert_eq!(v["url"], PAGE);
    assert_eq!(v["title"], "Flare test page");
    assert_eq!(v["image"]["mime"], "image/png");
    assert_eq!(v["image"]["width"], 800);
    assert_eq!(
        v["image"]["scale"],
        json!(1.333),
        "800 picture px for 600 css px"
    );
    let names: Vec<&str> = v["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Buy", "Quantity"]);
    assert_eq!(f.ctl.borrow().web.agent_refs(&id), 2);
    assert!(
        f.ctl.borrow().web.agent_op(&id).is_none(),
        "the call is over"
    );
    // The one script is the fixed pixel-ratio question, word for word.
    for (m, p) in cdp_calls(&slot) {
        if m == "Runtime.evaluate" {
            let p: Value = serde_json::from_str(&p).unwrap();
            assert_eq!(p["expression"], "window.devicePixelRatio");
        }
    }
    // Nothing but protocol methods was sent to the page.
    for (m, _) in cdp_calls(&slot) {
        assert!(
            [
                "DOM.enable",
                "Page.getLayoutMetrics",
                "Accessibility.getFullAXTree",
                "Page.captureScreenshot",
                "DOM.getContentQuads",
                "Runtime.evaluate"
            ]
            .contains(&m.as_str()),
            "{m}"
        );
    }
}

#[test]
fn a_look_of_a_tab_that_is_behind_has_the_list_but_no_picture() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    with_shop(&slot);
    // In front, the page is pictured.
    let rx = look(&f, &id);
    f.pump();
    assert!(rx.try_recv().unwrap().unwrap()["image"].is_object());
    // A second tab takes the front once the agent has left the page alone: it is hidden and paints nothing.
    f.ctl
        .borrow_mut()
        .web
        .skip_clock(crate::web_ctl::PARK_AWAKE_MS);
    assert_eq!(f.key(ctrl("t", "KeyT")), KeyOutcome::Consumed);
    f.h.advance(0);
    f.pump();
    assert_eq!(last_visible(&slot, &id), Some(false));
    let rx = look(&f, &id);
    f.pump();
    let v = rx.try_recv().unwrap().unwrap();
    assert!(v.get("image").is_none());
    assert!(v["image_error"].as_str().unwrap().contains("not on screen"));
    assert_eq!(v["elements"].as_array().unwrap().len(), 2);
    assert!(
        cdp_calls(&slot)
            .iter()
            .filter(|(m, _)| m == "Page.captureScreenshot")
            .count()
            == 1,
        "no picture was asked of the hidden page"
    );
}

#[test]
fn a_click_by_ref_uses_the_numbers_of_the_last_look() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    with_shop(&slot);
    let rx = look(&f, &id);
    f.pump();
    rx.try_recv().unwrap().unwrap();
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    // The cursor glides to the button first; the agent's timer lets the click go.
    agent_turn(&f, GLIDE_MAX_MS);
    let v = rx.try_recv().unwrap().unwrap();
    assert_eq!(v["clicked"], json!({"x": 140.0, "y": 110.0}));
    let presses: Vec<String> = cdp_calls(&slot)
        .into_iter()
        .filter(|(m, _)| m == "Input.dispatchMouseEvent")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(presses.len(), 3);
    assert!(presses[1].contains("mousePressed") && presses[1].contains("\"x\":140.0"));
}

/// `shop`, but the page says the point is under something else.
fn shop_covered(method: &str, params: &str) -> Result<String, String> {
    match method {
        "DOM.resolveNode" => Ok(json!({"object": {"objectId": "el-1"}}).to_string()),
        "Runtime.callFunctionOn" => Ok(json!({"result": {"type": "object", "value":
            {"covered": true, "tag": "div", "name": "Cookie\nwall"}}})
        .to_string()),
        _ => shop(method, params),
    }
}

#[test]
fn a_covered_click_is_refused_and_presses_nothing() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    with_shop(&slot);
    let rx = look(&f, &id);
    f.pump();
    rx.try_recv().unwrap().unwrap();
    fake(&slot).state.borrow_mut().cdp = Some(Box::new(shop_covered));
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    agent_turn(&f, GLIDE_MAX_MS);
    let e = rx.try_recv().unwrap().unwrap_err();
    assert_eq!(e.code, code::COVERED);
    assert_eq!(e.message, "covered by div: Cookie wall");
    assert!(
        cdp_calls(&slot)
            .iter()
            .all(|(m, _)| m != "Input.dispatchMouseEvent"),
        "nothing was clicked"
    );
    assert!(
        cdp_calls(&slot)
            .iter()
            .any(|(m, p)| m == "Runtime.releaseObject" && p.contains("el-1")),
        "the element held for the check is let go"
    );
    assert!(
        f.ctl.borrow().web.agent_op(&id).is_none(),
        "the call is over"
    );
    assert_eq!(f.ctl.borrow().web.agent_refs(&id), 2, "the refs still hold");
    assert_eq!(
        f.ctl.borrow().web.cursor_probe()["presses"],
        json!(0),
        "no ripple: nothing was pressed"
    );
}

#[test]
fn a_ref_is_forgotten_when_the_page_loads_something_new() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    with_shop(&slot);
    let rx = look(&f, &id);
    f.pump();
    rx.try_recv().unwrap().unwrap();
    assert_eq!(f.ctl.borrow().web.agent_refs(&id), 2);
    finish(&fake(&slot), &id, TWO, true);
    f.pump();
    assert_eq!(f.ctl.borrow().web.agent_refs(&id), 0);
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    let e = rx.try_recv().unwrap().unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
    assert!(e.message.contains("look again"));
    assert!(
        cdp_calls(&slot)
            .iter()
            .all(|(m, _)| m != "Input.dispatchMouseEvent"),
        "nothing was clicked"
    );
}

#[test]
fn the_page_is_in_use_while_the_call_runs_and_the_person_taking_over_ends_the_call() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    assert_eq!(f.ctl.borrow().web.using_text(&id), "");
    // No header and no ribbon: the address row is the panel's top, before, during and after the call.
    assert_eq!(f.h.comp.get_panel_header_h(), 0.0);
    // The page does not answer: the call is still running.
    let rx = look(&f, &id);
    f.pump();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    assert_eq!(f.ctl.borrow().web.agent_op(&id), Some("look"));
    assert_eq!(
        f.ctl.borrow().web.using_text(&id),
        "An agent is using this page"
    );
    assert_eq!(f.h.comp.get_panel_header_h(), 0.0);
    // The person clicks or types in the page.
    f.ctl.borrow_mut().web_user_input();
    f.pump();
    let e = rx.try_recv().unwrap().unwrap_err();
    assert_eq!(e.code, code::INTERRUPTED);
    assert!(e.message.contains("took over"));
    assert_eq!(f.ctl.borrow().web.using_text(&id), "");
    assert!(f.ctl.borrow().web.agent_op(&id).is_none());
    assert_eq!(f.ctl.borrow().web.agent_refs(&id), 0);
}

#[test]
fn the_hold_lasts_after_a_call_that_finished_until_the_person_acts() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    with_shop(&slot);
    let rx = look(&f, &id);
    f.pump();
    rx.try_recv().unwrap().unwrap();
    f.h.advance(0);
    assert_eq!(
        f.ctl.borrow().web.using_text(&id),
        "An agent is using this page",
        "still on right after the call"
    );
    // The person's own input ends it at once.
    f.ctl.borrow_mut().web_user_input();
    f.pump();
    assert_eq!(f.ctl.borrow().web.using_text(&id), "");
}

#[test]
fn a_second_call_while_one_runs_is_refused_and_the_first_is_not_disturbed() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let first = look(&f, &id);
    f.pump();
    let second = look(&f, &id);
    f.pump();
    let e = second.try_recv().unwrap().unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    assert!(e.message.contains("still running"));
    assert!(matches!(first.try_recv(), Err(TryRecvError::Empty)));
    assert_eq!(f.ctl.borrow().web.agent_op(&id), Some("look"));
}

#[test]
fn a_call_with_no_web_panel_says_how_to_open_one() {
    let (f, _h, _) = live(Opts::default());
    let _slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let rx = look(&f, &id);
    f.pump();
    let e = rx.try_recv().unwrap().unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    assert!(e.message.contains("open_page"));
    let rx = look(&f, "no-such-tab");
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::NO_TAB);
}

#[test]
fn closing_the_panel_answers_the_call_that_was_running() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let rx = look(&f, &id);
    f.pump();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    f.h.comp.invoke_panel_close();
    f.h.advance(0);
    f.pump();
    let e = rx.try_recv().unwrap().unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    assert!(e.message.contains("closed"));
    assert_eq!(f.ctl.borrow().web.using_text(&id), "");
}

#[test]
fn a_page_whose_process_ends_answers_the_call_too() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let rx = look(&f, &id);
    f.pump();
    fake(&slot).say(WebEvent::Gone {
        tab: id.clone(),
        why: "the page's process ended".into(),
    });
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::UNAVAILABLE);
}

#[test]
fn a_wait_asks_the_page_with_the_fixed_checker_only() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    fake(&slot).state.borrow_mut().cdp = Some(Box::new(|m, _| {
        assert_eq!(m, "Runtime.evaluate");
        Ok(json!({"result": {"type": "boolean", "value": true}}).to_string())
    }));
    let hostile = "\");alert(1);//";
    let rx = ask(
        &f,
        agent_op(
            &id,
            AgentOp::Wait {
                what: WaitFor::Text(hostile.into()),
                timeout_ms: 1_000,
            },
        ),
        Duration::from_secs(5),
    );
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap(), json!({"met": true}));
    let calls = cdp_calls(&slot);
    assert_eq!(calls.len(), 1);
    let p: Value = serde_json::from_str(&calls[0].1).unwrap();
    let expr = p["expression"].as_str().unwrap();
    let lit = serde_json::to_string(hostile).unwrap();
    assert_eq!(
        expr.replacen(&lit, "@", 1),
        flare_surfaces::agent_web::WAIT_TEXT_TEMPLATE
    );
}

#[test]
fn an_agent_in_a_tab_behind_gets_a_hidden_page_and_does_not_loop_on_open_page() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let a = f.ctl.borrow().front_id().unwrap();
    let _b = f.ctl.borrow_mut().open_folder(r"C:\b");
    f.ctl.borrow_mut().refresh();
    // open_page in the tab behind stores the page; no browser yet.
    let rx = ask(&f, open_op(&a, PAGE), Duration::from_secs(5));
    assert_eq!(rx.try_recv().unwrap().unwrap()["loaded"], false);
    assert!(slot.borrow().is_none());
    // The agent's look makes the browser, hidden, and says the page is not ready: not "open_page".
    let rx = look(&f, &a);
    f.pump();
    let e = rx.try_recv().unwrap().unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    assert!(!e.message.contains("no web page is open"), "{}", e.message);
    assert!(!e.message.contains("open_page"), "{}", e.message);
    assert_eq!(f.ctl.borrow().web_controllers(), 1);
    let o = opened_calls(&slot);
    let Some(Call::Open { tab, visible, .. }) = o.first() else {
        panic!("{o:?}")
    };
    assert_eq!((tab.as_str(), *visible), (a.as_str(), false));
    // Once it has loaded, the look works there (the list, no picture).
    finish(&fake(&slot), &a, PAGE, true);
    f.pump();
    with_shop(&slot);
    let rx = look(&f, &a);
    f.pump();
    let v = rx.try_recv().unwrap().unwrap();
    assert_eq!(v["elements"].as_array().unwrap().len(), 2);
    assert!(v.get("image").is_none());
    // It was never on screen while behind: it paints for the agent only off the window (issue #120).
    if last_visible(&slot, &a) == Some(true) {
        assert_eq!(
            last_bounds(&slot, &a).map(|r| (r.x, r.y)),
            Some((crate::web_ctl::PARK_AT, crate::web_ctl::PARK_AT))
        );
    }
}

/// Answers every DevTools call sent so far that has not been answered, from the stand-in shop page.
fn answer_all(f: &Fx, slot: &Slot, tab: &str, done: &mut std::collections::HashSet<u64>) {
    for c in fake(slot).calls() {
        if let Call::Cdp(_, id, m, p) = c
            && done.insert(id)
        {
            fake(slot).say(WebEvent::Cdp {
                tab: tab.into(),
                id,
                result: shop(&m, &p),
            });
        }
    }
    f.pump();
}

#[test]
fn a_look_that_finishes_after_a_navigation_does_not_put_back_the_old_refs() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    // No answers on their own: the test plays the browser, one step at a time.
    let rx = look(&f, &id);
    f.pump();
    let mut done = std::collections::HashSet::new();
    // The tree and the layout come back; the boxes and the picture are asked for.
    answer_all(&f, &slot, &id, &mut done);
    assert!(
        matches!(rx.try_recv(), Err(TryRecvError::Empty)),
        "still waiting for the boxes"
    );
    // The page starts a new navigation (the agent's click on a link, say).
    fake(&slot).say(WebEvent::Loading {
        tab: id.clone(),
        url: TWO.into(),
        nav: 900,
    });
    f.pump();
    // The old document's boxes arrive and the look ends.
    answer_all(&f, &slot, &id, &mut done);
    assert!(rx.try_recv().is_ok(), "the look still answers");
    assert_eq!(
        f.ctl.borrow().web.agent_refs(&id),
        0,
        "the names of the old page are not kept"
    );
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::NOT_FOUND);
}

#[test]
fn quitting_answers_a_call_that_was_running() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let rx = look(&f, &id);
    f.pump();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    assert_eq!(
        f.ctl.borrow_mut().on_close_requested(),
        CloseDecision::Close
    );
    let e = rx.try_recv().expect("answered").unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    assert!(e.message.contains("closing"));
}

#[test]
fn the_person_taking_over_a_click_lets_go_of_the_button() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    // The browser answers the page's size and never the first event: the press and the release are still
    // queued once the glide is over.
    let rx = ask(
        &f,
        agent_op(
            &id,
            AgentOp::Click {
                target: flare_surfaces::agent_job::Target::Point(10.0, 20.0),
                button: Button::Left,
                double: false,
            },
        ),
        Duration::from_secs(5),
    );
    f.pump();
    answer_all(&f, &slot, &id, &mut std::collections::HashSet::new());
    agent_turn(&f, GLIDE_MAX_MS);
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    f.ctl.borrow_mut().web_user_input();
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::INTERRUPTED);
    let sent: Vec<String> = cdp_calls(&slot)
        .into_iter()
        .filter(|(m, _)| m == "Input.dispatchMouseEvent")
        .map(|(_, p)| p)
        .collect();
    assert!(
        sent.last().is_some_and(|p| p.contains("mouseReleased")),
        "{sent:?}"
    );
}

// ---- the agent's cursor ----

use flare_surfaces::agent_cursor::{FADE_MS, GLIDE_MAX_MS, Point, glide_ms};
use flare_win::agent_cursor::{recorded, recorded_border};

/// A look of the shop, done.
fn looked(f: &Fx, slot: &Slot, id: &str) {
    with_shop(slot);
    let rx = look(f, id);
    f.pump();
    rx.try_recv().unwrap().unwrap();
}

/// Where the cursor's tip is in client pixels, while it is drawn.
fn cursor_at(f: &Fx) -> Option<(i64, i64)> {
    let p = f.ctl.borrow().web.cursor_probe();
    let c = &p["client"];
    (p["shown"] == json!(true)).then(|| (c[0].as_i64().unwrap(), c[1].as_i64().unwrap()))
}

/// Where a CSS point of the shop's 600 x 400 viewport is in client pixels of the page's rectangle.
fn on_page(f: &Fx, x: f64, y: f64) -> (i64, i64) {
    let r = drawn(f);
    (
        i64::from(r.x) + (x * f64::from(r.w) / 600.0).round() as i64,
        i64::from(r.y) + (y * f64::from(r.h) / 400.0).round() as i64,
    )
}

#[test]
fn the_cursor_shows_where_a_click_lands_before_it_presses_and_ripples_on_the_press() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    looked(&f, &slot, &id);
    assert_eq!(cursor_at(&f), None, "a look moves nothing");
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    // The point is known and the arrow is there; nothing is pressed yet.
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    assert_eq!(cursor_at(&f), Some(on_page(&f, 140.0, 110.0)));
    assert!(recorded().shown, "the shell was told");
    let pressed = |slot: &Slot| {
        cdp_calls(slot)
            .iter()
            .filter(|(_, p)| p.contains("mousePressed"))
            .count()
    };
    assert_eq!(pressed(&slot), 0);
    assert_eq!(f.ctl.borrow().web.cursor_probe()["tag"], "Agent");
    // The glide is over: the click goes, with a ripple.
    agent_turn(&f, GLIDE_MAX_MS);
    assert!(rx.try_recv().unwrap().is_ok());
    assert_eq!(pressed(&slot), 1);
    assert_eq!(f.ctl.borrow().web.cursor_probe()["presses"], 1);
    // It stays while the hold lasts, then fades and is gone.
    agent_turn(&f, crate::web_ctl::AGENT_HOLD_MS / 2);
    assert_eq!(cursor_at(&f), Some(on_page(&f, 140.0, 110.0)));
    agent_turn(&f, crate::web_ctl::AGENT_HOLD_MS);
    agent_turn(&f, FADE_MS / 2);
    let a = recorded().alpha;
    assert!(a > 0 && a < 255, "fading ({a})");
    agent_turn(&f, FADE_MS);
    assert_eq!(cursor_at(&f), None);
    assert!(!recorded().shown, "gone after the hold");
}

#[test]
fn the_next_action_glides_from_where_the_cursor_was() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    looked(&f, &slot, &id);
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    agent_turn(&f, GLIDE_MAX_MS);
    rx.try_recv().unwrap().unwrap();
    // Typing into the field: the arrow starts at the button and arrives at the field.
    let rx = ask(
        &f,
        agent_op(
            &id,
            AgentOp::Type {
                text: "3".into(),
                target: Some("e2".into()),
            },
        ),
        Duration::from_secs(5),
    );
    f.pump();
    assert_eq!(
        cursor_at(&f),
        Some(on_page(&f, 140.0, 110.0)),
        "it starts where it was"
    );
    // The typing waits as long as the glide takes, which is the distance's, not the longest (the
    // agent's clock also runs in real time, so only the halves are checked).
    let ms = glide_ms(css(140.0, 110.0), css(200.0, 160.0));
    assert!(ms < GLIDE_MAX_MS, "{ms}");
    agent_turn(&f, ms / 2);
    assert!(
        matches!(rx.try_recv(), Err(TryRecvError::Empty)),
        "still gliding"
    );
    let mid = cursor_at(&f).unwrap();
    assert!(
        mid != on_page(&f, 140.0, 110.0) && mid != on_page(&f, 200.0, 160.0),
        "{mid:?}"
    );
    agent_turn(&f, ms - ms / 2);
    assert_eq!(rx.try_recv().unwrap().unwrap()["typed"], 1);
    assert_eq!(cursor_at(&f), Some(on_page(&f, 200.0, 160.0)));
    assert_eq!(
        f.ctl.borrow().web.cursor_probe()["presses"],
        1,
        "typing presses nothing"
    );
}

/// A point of the shop's 600 x 400 viewport.
fn css(x: f64, y: f64) -> Point {
    Point {
        x,
        y,
        vp_w: 600.0,
        vp_h: 400.0,
    }
}

/// A click on the shop's button by ref, its glide and the hold over and the arrow faded.
fn click_and_fade(f: &Fx, id: &str) {
    let rx = click_ref(f, id, "e1");
    f.pump();
    agent_turn(f, GLIDE_MAX_MS);
    rx.try_recv().unwrap().unwrap();
    agent_turn(f, crate::web_ctl::AGENT_HOLD_MS);
    agent_turn(f, FADE_MS);
    assert_eq!(cursor_at(f), None, "faded");
}

fn type_into_field(f: &Fx, id: &str) -> Receiver<Result<Value, flare_ctl::CtlError>> {
    ask(
        f,
        agent_op(
            id,
            AgentOp::Type {
                text: "3".into(),
                target: Some("e2".into()),
            },
        ),
        Duration::from_secs(5),
    )
}

#[test]
fn after_the_fade_the_next_action_glides_on_from_where_the_arrow_rested() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    looked(&f, &slot, &id);
    click_and_fade(&f, &id);
    assert_eq!(f.ctl.borrow().web.cursor_probe()["remembers"], true);
    // Seconds later the next action: the arrow is back at the button, not at the field, and glides.
    agent_turn(&f, 3_000);
    let rx = type_into_field(&f, &id);
    f.pump();
    assert_eq!(
        cursor_at(&f),
        Some(on_page(&f, 140.0, 110.0)),
        "no jump: it starts where it rested"
    );
    let ms = glide_ms(css(140.0, 110.0), css(200.0, 160.0));
    agent_turn(&f, ms / 2);
    assert!(
        matches!(rx.try_recv(), Err(TryRecvError::Empty)),
        "still gliding"
    );
    agent_turn(&f, ms - ms / 2);
    assert_eq!(rx.try_recv().unwrap().unwrap()["typed"], 1);
    assert_eq!(cursor_at(&f), Some(on_page(&f, 200.0, 160.0)));
}

#[test]
fn a_new_page_or_a_takeover_forgets_where_the_arrow_rested() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    looked(&f, &slot, &id);
    click_and_fade(&f, &id);
    // The page loads something new: the next action appears at its target.
    finish(&fake(&slot), &id, TWO, true);
    f.pump();
    looked(&f, &slot, &id);
    let rx = type_into_field(&f, &id);
    f.pump();
    assert_eq!(cursor_at(&f), Some(on_page(&f, 200.0, 160.0)), "appears");
    agent_turn(&f, GLIDE_MAX_MS);
    rx.try_recv().unwrap().unwrap();
    // The person takes over while it holds: gone, and nothing is remembered.
    f.ctl.borrow_mut().web_user_input();
    f.pump();
    assert_eq!(cursor_at(&f), None);
    assert_eq!(f.ctl.borrow().web.cursor_probe()["remembers"], false);
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    assert_eq!(cursor_at(&f), Some(on_page(&f, 140.0, 110.0)), "appears");
    agent_turn(&f, GLIDE_MAX_MS);
    rx.try_recv().unwrap().unwrap();
}

/// The border as the probe reports it: drawn, its rectangle in client pixels and its opacity.
fn border(f: &Fx) -> (bool, Value, Value) {
    let p = f.ctl.borrow().web.cursor_probe();
    let b = &p["border"];
    (
        b["shown"] == json!(true),
        b["client"].clone(),
        b["alpha"].clone(),
    )
}

#[test]
fn a_light_orange_border_rings_the_page_while_the_agent_uses_it_and_fades_with_the_arrow() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    assert!(!border(&f).0, "no agent, no border");
    // The page does not answer the look: the call runs, and the border is up from its start.
    let rx = look(&f, &id);
    f.pump();
    let (shown, rect, alpha) = border(&f);
    assert!(shown);
    let r = drawn(&f);
    assert_eq!(
        rect,
        json!([r.x, r.y, r.w, r.h]),
        "the page's own rectangle"
    );
    assert_eq!(alpha, 255);
    let rec = recorded_border();
    assert!(rec.shown);
    assert_eq!((rec.w, rec.h), (r.w as u32, r.h as u32));
    // The person takes over: gone at once, no fade.
    f.ctl.borrow_mut().web_user_input();
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::INTERRUPTED);
    assert!(!border(&f).0);
    assert!(!recorded_border().shown);
    // A finished click: up through the hold, then it fades with the arrow and is gone.
    looked(&f, &slot, &id);
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    agent_turn(&f, GLIDE_MAX_MS);
    rx.try_recv().unwrap().unwrap();
    agent_turn(&f, crate::web_ctl::AGENT_HOLD_MS - 100);
    assert_eq!(border(&f).2, 255, "the hold lasts");
    // The hold ends: the border and the arrow start to fade together.
    agent_turn(&f, 100);
    let draws = recorded_border().draws;
    agent_turn(&f, FADE_MS / 2);
    let a = border(&f).2.as_u64().unwrap();
    assert!(a > 0 && a < 255, "fading ({a})");
    assert!(cursor_at(&f).is_some(), "with the arrow");
    assert_eq!(
        recorded_border().draws,
        draws,
        "a fade changes only the opacity"
    );
    agent_turn(&f, FADE_MS);
    assert!(!border(&f).0);
    assert!(!recorded_border().shown);
    assert_eq!(cursor_at(&f), None);
    // The next action brings it back whole.
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    assert_eq!(border(&f).2, 255);
    agent_turn(&f, GLIDE_MAX_MS);
    rx.try_recv().unwrap().unwrap();
}

/// Every rectangle sent to the page, oldest first.
fn all_bounds(slot: &Slot, tab: &str) -> Vec<PhysRect> {
    fake(slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Bounds(t, r) if t == tab => Some(r),
            _ => None,
        })
        .collect()
}

/// Takes the splitter of the open page; the slot's width and the page's width in pixels at that moment.
fn take_splitter(f: &Fx) -> (f32, i32) {
    let c = &f.h.comp;
    let w = (c.get_panel_slot_w(), drawn(f).w);
    c.set_panel_drag_w(w.0);
    c.set_panel_splitting(true);
    f.h.advance(0);
    f.pump();
    w
}

fn splitter_to(f: &Fx, w: f32) {
    f.h.comp.set_panel_drag_w(w);
    f.h.advance(0);
    f.pump();
}

#[test]
fn a_held_splitter_moves_the_pages_edge_but_never_its_width_until_it_is_let_go() {
    for scale in [1.0f32, 2.25] {
        let (f, _h, _) = live(Opts {
            scale,
            ..Opts::default()
        });
        let slot = with_fake(&f);
        let id = up(&f, &slot);
        f.h.advance(0);
        let (w0, page) = take_splitter(&f);
        let before = all_bounds(&slot, &id).len();
        // Narrower and then wider than where it began: the page is never resized, and never over the terminal.
        for (step, css) in [-24.0f32, -48.0, -72.0, 24.0, 72.0].into_iter().enumerate() {
            splitter_to(&f, w0 + css);
            let want = PhysRect {
                w: page,
                ..drawn(&f)
            };
            assert_ne!(
                drawn(&f).w,
                page,
                "scale {scale}, step {step}: the slot moved"
            );
            assert_eq!(
                last_bounds(&slot, &id),
                Some(want),
                "scale {scale}, step {step}"
            );
        }
        assert!(
            all_bounds(&slot, &id)[before..].iter().all(|r| r.w == page),
            "scale {scale}: no width but the one it had"
        );
        assert!(crate::timer_registry::running(crate::web_ctl::DRAG_TIMER));
        // Let go: exactly the slot, at once, and the timer is off.
        f.h.comp.invoke_end_splitter();
        f.h.advance(0);
        f.pump();
        assert_eq!(last_bounds(&slot, &id), Some(drawn(&f)), "scale {scale}");
        assert_ne!(drawn(&f).w, page, "scale {scale}");
        assert!(!crate::timer_registry::running(crate::web_ctl::DRAG_TIMER));
        // A settle that was already on its way changes nothing.
        let sent = all_bounds(&slot, &id).len();
        f.ctl
            .borrow_mut()
            .on_msg(UiMsg::Web(WebMsgBox(WebMsg::DragSettled)));
        f.pump();
        assert_eq!(all_bounds(&slot, &id).len(), sent, "scale {scale}");
    }
}

#[test]
fn a_pause_in_a_splitter_drag_fits_the_page_once_and_the_next_step_holds_again() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    let (w0, page) = take_splitter(&f);
    splitter_to(&f, w0 + 40.0);
    assert_eq!(last_bounds(&slot, &id).map(|r| r.w), Some(page));
    let sent = all_bounds(&slot, &id).len();
    // Not yet: the wait is not over.
    f.h.advance(crate::web_ctl::DRAG_SETTLE_MS / 2);
    f.pump();
    assert_eq!(all_bounds(&slot, &id).len(), sent, "still waiting");
    // A step starts the wait again.
    splitter_to(&f, w0 + 80.0);
    f.h.advance(crate::web_ctl::DRAG_SETTLE_MS / 2);
    f.pump();
    assert_eq!(
        last_bounds(&slot, &id).map(|r| r.w),
        Some(page),
        "restarted"
    );
    let sent = all_bounds(&slot, &id).len();
    f.h.advance(crate::web_ctl::DRAG_SETTLE_MS);
    f.pump();
    let fitted = drawn(&f);
    assert_eq!(last_bounds(&slot, &id), Some(fitted), "the pause fits it");
    assert_eq!(all_bounds(&slot, &id).len(), sent + 1, "once");
    assert!(
        f.h.comp.get_panel_splitting(),
        "and the splitter is still held"
    );
    // The page holds its new width from here.
    splitter_to(&f, w0 + 110.0);
    assert_eq!(
        last_bounds(&slot, &id),
        Some(PhysRect {
            w: fitted.w,
            ..drawn(&f)
        })
    );
}

#[test]
fn a_splitter_grab_that_goes_away_leaves_the_page_on_the_slot() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    let (w0, page) = take_splitter(&f);
    let at_rest = drawn(&f);
    splitter_to(&f, w0 + 60.0);
    assert_eq!(last_bounds(&slot, &id).map(|r| r.w), Some(page));
    // No release and no share: the slot is back where it was, and so is the page.
    f.h.comp.set_panel_splitting(false);
    f.h.advance(0);
    f.pump();
    assert_eq!(drawn(&f), at_rest);
    assert_eq!(last_bounds(&slot, &id), Some(at_rest));
    assert!(!crate::timer_registry::running(crate::web_ctl::DRAG_TIMER));
}

#[test]
fn a_panel_closed_during_a_splitter_drag_comes_back_at_the_slots_size() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    let (w0, _) = take_splitter(&f);
    splitter_to(&f, w0 + 60.0);
    f.h.comp.invoke_panel_close();
    f.h.advance(0);
    f.pump();
    assert!(!f.h.comp.get_panel_splitting());
    assert!(!crate::timer_registry::running(crate::web_ctl::DRAG_TIMER));
    assert_eq!(up(&f, &slot), id);
    f.h.advance(0);
    f.pump();
    assert_eq!(last_bounds(&slot, &id), Some(drawn(&f)));
}

#[test]
fn a_window_resize_moves_the_page_at_every_step_with_no_hold() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    let size = f.h.comp.window().size();
    for step in 1..=3u32 {
        f.h.comp.window().set_size(slint::PhysicalSize::new(
            size.width - 30 * step,
            size.height - 10 * step,
        ));
        f.h.advance(0);
        f.pump();
        assert_eq!(last_bounds(&slot, &id), Some(drawn(&f)), "step {step}");
        assert!(!crate::timer_registry::running(crate::web_ctl::DRAG_TIMER));
    }
    f.h.comp.window().set_size(size);
}

#[test]
fn a_held_splitter_takes_the_glow_away_while_the_page_holds_its_width_and_it_comes_back_after() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    let _rx = look(&f, &id);
    f.pump();
    assert!(border(&f).0, "the glow is up while the agent works");
    let c = &f.h.comp;
    let w0 = c.get_panel_slot_w();
    let page = drawn(&f).w;
    c.set_panel_drag_w(w0);
    c.set_panel_splitting(true);
    f.h.advance(0);
    f.pump();
    assert!(!border(&f).0, "off the page while the splitter is held");
    assert!(!recorded_border().shown);
    let draws = recorded_border().draws;
    for step in 1..=5 {
        c.set_panel_drag_w(w0 - 8.0 * step as f32);
        f.h.advance(0);
        f.pump();
        // The page's edge follows each step at once, at the width it had; the glow is not drawn again for it.
        let held = PhysRect {
            w: page,
            ..drawn(&f)
        };
        assert_eq!(last_bounds(&slot, &id), Some(held), "step {step}");
        assert!(!border(&f).0);
        assert_eq!(recorded_border().draws, draws, "step {step}");
    }
    // Let go: the glow is back, once, at the page's new size.
    c.invoke_end_splitter();
    f.h.advance(0);
    f.pump();
    let (shown, rect, _) = border(&f);
    let r = drawn(&f);
    assert!(shown, "back after the drag");
    assert_eq!(rect, json!([r.x, r.y, r.w, r.h]));
    assert_eq!(recorded_border().draws, draws + 1);
    assert_eq!(
        (recorded_border().w, recorded_border().h),
        (r.w as u32, r.h as u32)
    );
}

#[test]
fn a_menu_over_the_page_takes_the_border_away_and_it_comes_back_after() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    let _rx = look(&f, &id);
    f.pump();
    assert!(border(&f).0);
    f.h.comp.set_dialog_open(true);
    f.h.advance(0);
    f.pump();
    assert!(!border(&f).0, "not drawn over the menu");
    f.h.comp.set_dialog_open(false);
    f.h.advance(0);
    f.pump();
    agent_turn(&f, 16);
    assert!(border(&f).0, "the call still runs");
}

#[test]
fn with_animation_effects_off_nothing_glides_or_fades() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.ctl
        .borrow_mut()
        .web
        .set_motion(std::rc::Rc::new(|| false));
    f.h.advance(0);
    looked(&f, &slot, &id);
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    assert!(rx.try_recv().unwrap().is_ok(), "no wait for a glide");
    assert_eq!(cursor_at(&f), Some(on_page(&f, 140.0, 110.0)));
    let rx = type_into_field(&f, &id);
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap()["typed"], 1);
    assert_eq!(
        cursor_at(&f),
        Some(on_page(&f, 200.0, 160.0)),
        "placed at once"
    );
    // After the hold: the arrow and the border go at once, with no fade.
    agent_turn(&f, crate::web_ctl::AGENT_HOLD_MS);
    assert_eq!(cursor_at(&f), None);
    assert!(!border(&f).0);
}

#[test]
fn the_person_taking_over_takes_the_cursor_away_at_once() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    looked(&f, &slot, &id);
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    assert!(cursor_at(&f).is_some());
    f.ctl.borrow_mut().web_user_input();
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::INTERRUPTED);
    assert_eq!(cursor_at(&f), None);
    assert!(!recorded().shown);
    // And a held cursor after a finished click goes with the hold when the person acts.
    looked(&f, &slot, &id);
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    agent_turn(&f, GLIDE_MAX_MS);
    rx.try_recv().unwrap().unwrap();
    assert!(cursor_at(&f).is_some());
    f.ctl.borrow_mut().web_user_input();
    f.pump();
    assert_eq!(cursor_at(&f), None);
}

#[test]
fn a_menu_over_the_page_or_a_tab_in_front_hides_the_cursor_and_a_hidden_page_does_not_wait() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    looked(&f, &slot, &id);
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    assert!(cursor_at(&f).is_some());
    // A Flare menu opens over the page: its still is drawn and the cursor goes.
    f.h.comp.set_dialog_open(true);
    f.h.advance(0);
    f.pump();
    assert_ne!(f.ctl.borrow().web.phase_name(), "live");
    assert_eq!(cursor_at(&f), None);
    agent_turn(&f, GLIDE_MAX_MS);
    assert!(rx.try_recv().unwrap().is_ok(), "the click still goes");
    assert_eq!(cursor_at(&f), None, "and is not drawn over the menu");
    f.h.comp.set_dialog_open(false);
    f.h.advance(0);
    f.pump();
    // A second tab in front: a click on the page behind has no cursor and no glide to wait for.
    assert_eq!(f.key(ctrl("t", "KeyT")), KeyOutcome::Consumed);
    f.h.advance(0);
    f.pump();
    looked(&f, &slot, &id);
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    assert!(rx.try_recv().unwrap().is_ok(), "answered with no wait");
    assert_eq!(cursor_at(&f), None);
}

/// Issue #120: a hidden page paints no frames, so what it does in its next frame (a menu opening, a switch
/// flipping) never happened, and a click there looked like it did nothing. A page behind that the agent works
/// on paints off the window while the call runs and for `PARK_AWAKE_MS` after, at the panel's size, then
/// rests hidden; back in front it is in its panel again.
#[test]
fn a_page_behind_paints_off_the_window_while_the_agent_works_on_it() {
    use crate::web_ctl::{PARK_AT, PARK_AWAKE_MS};
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    let panel = last_bounds(&slot, &id).expect("the page has its place");
    let _b = f.ctl.borrow_mut().open_folder("C:\\b");
    f.ctl.borrow_mut().refresh();
    f.h.advance(0);
    f.pump();
    assert_eq!(
        last_visible(&slot, &id),
        Some(false),
        "behind, it is hidden"
    );
    looked(&f, &slot, &id);
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    assert!(rx.try_recv().unwrap().is_ok(), "the click is answered");
    let parked = last_bounds(&slot, &id).unwrap();
    assert_eq!(
        (parked.x, parked.y, parked.w, parked.h),
        (PARK_AT, PARK_AT, panel.w, panel.h),
        "off the window, at the panel's size"
    );
    assert_eq!(last_visible(&slot, &id), Some(true), "and painting");
    agent_turn(&f, PARK_AWAKE_MS - 100);
    assert_eq!(
        last_visible(&slot, &id),
        Some(true),
        "a moment after the call it still paints"
    );
    agent_turn(&f, 200);
    assert_eq!(
        last_visible(&slot, &id),
        Some(false),
        "then it rests hidden"
    );
    // Back in front it is in its panel and on screen.
    assert_eq!(
        f.key(KeyEvent::new("1", "Digit1").ctrl()),
        KeyOutcome::Consumed
    );
    f.h.advance(0);
    f.pump();
    assert_eq!(last_visible(&slot, &id), Some(true));
    assert_eq!(last_bounds(&slot, &id), Some(panel));
}

/// A page in front, live, is never moved off the window for the agent: it is used where it is.
#[test]
fn a_page_in_front_is_not_parked() {
    use crate::web_ctl::PARK_AT;
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    looked(&f, &slot, &id);
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    agent_turn(&f, GLIDE_MAX_MS);
    assert!(rx.try_recv().unwrap().is_ok());
    assert!(
        !fake(&slot)
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Bounds(t, r) if *t == id && r.x == PARK_AT)),
        "never sent off the window"
    );
    assert_eq!(last_visible(&slot, &id), Some(true));
}

/// A Flare menu opens over the page in front while the agent works on it: the page stays in its panel until its
/// still is there (no blank panel), and is then hidden for real, which gives the keyboard back to the window. Only
/// after that may it paint off the window for the agent.
#[test]
fn a_menu_over_the_page_the_agent_uses_keeps_it_in_place_until_the_still() {
    use crate::web_ctl::PARK_AT;
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    let panel = last_bounds(&slot, &id).expect("the page has its place");
    looked(&f, &slot, &id);
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    fake(&slot).state.borrow_mut().still_silent = true;
    f.h.comp.set_dialog_open(true);
    f.h.advance(0);
    f.pump();
    assert_eq!(f.ctl.borrow().web.phase_name(), "capturing");
    assert_eq!(
        last_bounds(&slot, &id),
        Some(panel),
        "in its panel while the still is taken"
    );
    assert_eq!(last_visible(&slot, &id), Some(true));
    let token = fake(&slot)
        .calls()
        .iter()
        .find_map(|c| match c {
            Call::Capture(t, k) if *t == id => Some(*k),
            _ => None,
        })
        .expect("a still was asked for");
    fake(&slot).say(WebEvent::Still {
        tab: id.clone(),
        token,
        png: None,
    });
    f.pump();
    assert_eq!(f.ctl.borrow().web.phase_name(), "still");
    let calls = fake(&slot).calls();
    let hide = calls
        .iter()
        .rposition(|c| *c == Call::Visible(id.clone(), false))
        .expect("the page is hidden once the still is there");
    let park = calls
        .iter()
        .position(|c| matches!(c, Call::Bounds(t, r) if *t == id && r.x == PARK_AT));
    assert!(
        park.is_none_or(|p| p > hide),
        "never sent off the window before it was hidden"
    );
    agent_turn(&f, GLIDE_MAX_MS);
    assert!(rx.try_recv().unwrap().is_ok(), "the click still goes");
    // The menu closes: the page is back in its panel, live.
    f.h.comp.set_dialog_open(false);
    f.h.advance(0);
    f.pump();
    assert_eq!(last_bounds(&slot, &id), Some(panel));
    assert_eq!(last_visible(&slot, &id), Some(true));
}

#[test]
fn closing_the_panel_takes_the_cursor_with_it() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    looked(&f, &slot, &id);
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    assert!(cursor_at(&f).is_some());
    f.h.comp.invoke_panel_close();
    f.h.advance(0);
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::UNAVAILABLE);
    assert_eq!(cursor_at(&f), None);
    assert!(!recorded().shown);
}

#[test]
fn the_person_focusing_the_page_while_a_call_runs_takes_over() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let rx = look(&f, &id);
    f.pump();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    fake(&slot).say(WebEvent::Focus {
        tab: id.clone(),
        has: true,
    });
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::INTERRUPTED);
}

#[test]
fn a_job_whose_caller_went_away_ends_and_the_page_is_free_for_the_next_call() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let cancel = flare_ctl::CancelFlag::default();
    let rx = ask(
        &f,
        CtlOp::BrowserAgent {
            tab: id.clone(),
            op: AgentOp::Look { picture: true },
            cancel: cancel.clone(),
        },
        Duration::from_secs(5),
    );
    f.pump();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    cancel.set();
    agent_turn(&f, 20);
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::INTERRUPTED);
    // The next call is not told another one still runs on the page.
    let next = look(&f, &id);
    f.pump();
    assert!(
        matches!(next.try_recv(), Err(TryRecvError::Empty)),
        "the cancelled job left the page in use"
    );
}

#[test]
fn a_job_cancelled_before_the_window_took_it_up_never_starts() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let cancel = flare_ctl::CancelFlag::default();
    cancel.set();
    let before = cdp_calls(&slot).len();
    let rx = ask(
        &f,
        CtlOp::BrowserAgent {
            tab: id,
            op: AgentOp::Look { picture: true },
            cancel,
        },
        Duration::from_secs(5),
    );
    f.pump();
    assert!(rx.try_recv().unwrap().is_err());
    assert_eq!(
        cdp_calls(&slot).len(),
        before,
        "nothing was sent to the page"
    );
}

// ---- mini-apps and diagrams (stage 5) ---------------------------------------------------------------

use flare_surfaces::app::{self, AppSpec};

/// An app written into a folder of this test's own, with the controller told where the apps live.
fn app_in(f: &Fx, seq: u64) -> (AppSpec, std::path::PathBuf) {
    let root = f.dir.path().join("apps");
    f.ctl.borrow_mut().set_apps_root(root.clone());
    let page = app::wrap_html("<p id=hi>hello</p>", false);
    let spec = app::write_app(&root, seq, &[(app::PAGE, page.as_bytes())], false).unwrap();
    (spec, root)
}

fn app_op(tab: &str, spec: &AppSpec, title: &str, diagram: bool) -> CtlOp {
    CtlOp::ShowApp {
        tab: tab.into(),
        spec: spec.clone(),
        title: title.into(),
        diagram,
    }
}

fn open_apps(slot: &Slot) -> Vec<(String, AppSpec, bool)> {
    fake(slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::OpenApp {
                tab, app, visible, ..
            } => Some((tab, app, visible)),
            _ => None,
        })
        .collect()
}

/// Shows an app on the tab and plays the browser loading it.
fn shown_app(f: &Fx, slot: &Slot, tab: &str, spec: &AppSpec, title: &str) -> Value {
    let rx = ask(f, app_op(tab, spec, title, false), Duration::from_secs(5));
    finish(&fake(slot), tab, &spec.url, true);
    f.pump();
    rx.try_recv().unwrap().unwrap()
}

#[test]
fn a_mini_app_gets_a_view_of_its_own_kind_and_answers_when_it_has_loaded() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let (spec, _root) = app_in(&f, 1);
    let rx = ask(
        &f,
        app_op(&id, &spec, "Chart", false),
        Duration::from_secs(5),
    );
    assert!(
        matches!(rx.try_recv(), Err(TryRecvError::Empty)),
        "waits for the load"
    );
    let o = open_apps(&slot);
    assert_eq!(o.len(), 1, "{o:?}");
    assert_eq!((o[0].0.as_str(), &o[0].1), (id.as_str(), &spec));
    // The plain open was never used for it: the app's view is a different kind.
    assert!(opened_calls(&slot).is_empty());
    finish(&fake(&slot), &id, &spec.url, true);
    f.pump();
    let v = rx.try_recv().unwrap().unwrap();
    assert_eq!(v["panel"]["kind"], "app");
    assert_eq!(v["panel"]["title"], "Chart");
    assert_eq!(v["panel"]["network"], false);
    assert!(
        v["panel"].get("url").is_none(),
        "the address is Flare's own"
    );
    assert_eq!(v["loaded"], true);
    // The header is the agent's title (a mini-app keeps its header), and there is no address bar.
    assert_eq!(f.h.comp.get_panel_title().as_str(), "Chart");
    assert!(f.h.comp.get_panel_web_app());
    assert_eq!(f.h.comp.get_panel_bar_h(), 0.0);
    assert!(f.h.comp.get_panel_header_h() > 0.0);
    assert_eq!(f.ctl.borrow().web_controllers(), 1);
}

#[test]
fn the_page_cannot_rename_the_panel_or_move_its_address() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let (spec, _root) = app_in(&f, 1);
    shown_app(&f, &slot, &id, &spec, "Chart");
    fake(&slot).say(WebEvent::Title {
        tab: id.clone(),
        title: "Totally the bank".into(),
    });
    fake(&slot).say(WebEvent::Url {
        tab: id.clone(),
        url: "https://example.com/login".into(),
    });
    f.pump();
    let state = ask(
        &f,
        CtlOp::PanelState {
            tab: id.clone(),
            cwd: None,
        },
        Duration::from_secs(5),
    );
    let v = state.try_recv().unwrap().unwrap();
    assert_eq!(v["panel"]["title"], "Chart");
    assert_eq!(
        f.ctl.borrow().panels.web_target(&id).as_deref(),
        Some(spec.url.as_str())
    );
    assert_eq!(f.h.comp.get_panel_title().as_str(), "Chart");
}

#[test]
fn the_address_field_and_the_channel_cannot_send_a_mini_app_anywhere() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let (spec, _root) = app_in(&f, 1);
    shown_app(&f, &slot, &id, &spec, "Chart");
    f.ctl
        .borrow_mut()
        .web_submit(&"https://example.com/".into());
    let nav = ask(
        &f,
        CtlOp::BrowserNavigate {
            tab: id.clone(),
            to: NavTo::Url("https://example.com/".into()),
        },
        Duration::from_secs(5),
    );
    assert_eq!(nav.try_recv().unwrap().unwrap_err().code, code::UNAVAILABLE);
    assert!(
        !fake(&slot)
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Navigate(..))),
        "nothing was sent to the view"
    );
}

#[test]
fn closing_a_mini_app_ends_it_and_removes_its_folder() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let (spec, _root) = app_in(&f, 1);
    shown_app(&f, &slot, &id, &spec, "Chart");
    assert!(spec.folder.is_dir());
    let closed = ask(
        &f,
        CtlOp::PanelClose { tab: id.clone() },
        Duration::from_secs(5),
    );
    assert_eq!(closed.try_recv().unwrap().unwrap()["closed"], true);
    f.h.advance(0);
    assert_eq!(f.ctl.borrow().web_controllers(), 0, "the view is freed");
    // The folder goes on a worker thread.
    f.pump_until("the folder to go", |_| !spec.folder.exists());
    // The toggle chord brings nothing back: there is no panel left to reopen.
    assert!(f.ctl.borrow().panels.app_spec(&id).is_none());
    assert_eq!(f.key(shift_ctrl("P", "KeyP")), KeyOutcome::Consumed);
    f.h.advance(0);
    f.pump();
    assert_eq!(f.ctl.borrow().web_controllers(), 0);
    assert_eq!(open_apps(&slot).len(), 1);
}

#[test]
fn a_page_replaces_a_mini_app_and_a_mini_app_replaces_a_page() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let (spec, _root) = app_in(&f, 1);
    // An app over a page: the page's view is closed and the app gets one of its own kind.
    let rx = ask(
        &f,
        app_op(&id, &spec, "Chart", false),
        Duration::from_secs(5),
    );
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    let calls = fake(&slot).calls();
    let close = calls
        .iter()
        .rposition(|c| *c == Call::Close(id.clone()))
        .expect("closed");
    let open = calls
        .iter()
        .position(|c| matches!(c, Call::OpenApp { .. }))
        .expect("app opened");
    assert!(close < open, "the page's view goes first: {calls:?}");
    finish(&fake(&slot), &id, &spec.url, true);
    f.pump();
    rx.try_recv().unwrap().unwrap();
    // A page over the app: the app's view and folder go and the page gets a plain view.
    let rx = ask(&f, open_op(&id, PAGE), Duration::from_secs(5));
    let calls = fake(&slot).calls();
    let last_close = calls
        .iter()
        .rposition(|c| *c == Call::Close(id.clone()))
        .unwrap();
    let last_open = calls
        .iter()
        .rposition(|c| matches!(c, Call::Open { .. }))
        .unwrap();
    assert!(last_close > close, "the app's view was closed: {calls:?}");
    assert!(last_close < last_open, "{calls:?}");
    finish(&fake(&slot), &id, PAGE, true);
    f.pump();
    assert!(rx.try_recv().unwrap().is_ok());
    assert!(f.ctl.borrow().panels.app_spec(&id).is_none());
    f.pump_until("the folder to go", |_| !spec.folder.exists());
    // The plain page's bar is back.
    assert!(!f.h.comp.get_panel_web_app());
}

#[test]
fn a_second_mini_app_replaces_the_first_and_its_folder() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let (one, _root) = app_in(&f, 1);
    shown_app(&f, &slot, &id, &one, "One");
    let (two, _root) = app_in(&f, 2);
    let v = shown_app(&f, &slot, &id, &two, "Two");
    assert_eq!(v["panel"]["title"], "Two");
    f.pump_until("the first folder to go", |_| !one.folder.exists());
    assert!(two.folder.is_dir());
    let o = open_apps(&slot);
    assert_eq!(o.len(), 2, "{:#?}", fake(&slot).calls());
    assert_ne!(o[0].1.host, o[1].1.host);
}

#[test]
fn a_mini_app_for_a_tab_behind_gets_its_view_hidden() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let a = f.ctl.borrow().front_id().unwrap();
    let _b = f.ctl.borrow_mut().open_folder(r"C:\b");
    f.ctl.borrow_mut().refresh();
    let (spec, _root) = app_in(&f, 1);
    let rx = ask(
        &f,
        app_op(&a, &spec, "Chart", false),
        Duration::from_secs(5),
    );
    let o = open_apps(&slot);
    assert_eq!(o.len(), 1, "the page loads even though nobody is looking");
    assert!(!o[0].2, "hidden");
    finish(&fake(&slot), &a, &spec.url, true);
    f.pump();
    rx.try_recv().unwrap().unwrap();
}

#[test]
fn a_mini_app_for_a_closed_tab_is_refused_and_its_folder_removed() {
    let (f, _h, _) = live(Opts::default());
    let _slot = with_fake(&f);
    let (spec, _root) = app_in(&f, 1);
    let rx = ask(
        &f,
        app_op("no-such-tab", &spec, "Chart", false),
        Duration::from_secs(5),
    );
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::NO_TAB);
    f.pump_until("the folder to go", |_| !spec.folder.exists());
}

#[test]
fn a_mini_app_asked_for_too_late_is_never_shown() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let (spec, _root) = app_in(&f, 1);
    let rx = ask(&f, app_op(&id, &spec, "Chart", false), Duration::ZERO);
    let e = rx.try_recv().unwrap().unwrap_err();
    assert_eq!(e.code, code::TIMEOUT);
    assert!(slot.borrow().is_none(), "no browser was made for it");
    f.pump_until("the folder to go", |_| !spec.folder.exists());
}

#[test]
fn a_diagram_is_answered_by_the_pages_word_and_a_drawing_that_failed_says_why() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let (spec, _root) = app_in(&f, 1);
    let rx = ask(&f, app_op(&id, &spec, "Flow", true), Duration::from_secs(5));
    finish(&fake(&slot), &id, &spec.url, true);
    f.pump();
    // Loaded is not drawn: the page is asked, and nothing is answered yet.
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    let asked: Vec<(u64, String)> = fake(&slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Cdp(_, id, m, p) if m == "Runtime.evaluate" => Some((id, p)),
            _ => None,
        })
        .collect();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(asked[0].1.contains("window.__flareDiagram"));
    assert!(asked[0].1.contains("\"awaitPromise\":true"));
    fake(&slot).say(WebEvent::Cdp {
        tab: id.clone(),
        id: asked[0].0,
        result: Ok(r#"{"result":{"type":"object","value":{"ok":true}}}"#.into()),
    });
    f.pump();
    let v = rx.try_recv().unwrap().unwrap();
    assert_eq!(v["panel"]["kind"], "app");
    assert_eq!(v["panel"]["title"], "Flow");

    // A diagram that does not parse: the call fails with Mermaid's own words (the panel shows them).
    let (bad, _root) = app_in(&f, 2);
    let rx = ask(
        &f,
        app_op(&id, &bad, "Broken", true),
        Duration::from_secs(5),
    );
    finish(&fake(&slot), &id, &bad.url, true);
    f.pump();
    let asked: Vec<u64> = fake(&slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Cdp(_, id, m, _) if m == "Runtime.evaluate" => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(asked.len(), 2);
    fake(&slot).say(WebEvent::Cdp {
        tab: id.clone(),
        id: asked[1],
        result: Ok(serde_json::json!({"result": {"type": "object", "value": {
            "ok": false,
            "error": "Parse error on line 2:\nA-->\n---^\nExpecting 'SEMI'"
        }}})
        .to_string()),
    });
    f.pump();
    let e = rx.try_recv().unwrap().unwrap_err();
    assert_eq!(e.code, code::BAD_PARAMS);
    assert!(
        e.message
            .starts_with("the diagram did not draw: Parse error on line 2:"),
        "{}",
        e.message
    );
    // The panel stays on the error page (it shows the text), still under the agent's title.
    assert_eq!(f.h.comp.get_panel_title().as_str(), "Broken");
}

#[test]
fn a_diagram_whose_page_failed_closed_or_moved_on_answers_unavailable() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    // The page failed to load.
    let (one, _root) = app_in(&f, 1);
    let rx = ask(&f, app_op(&id, &one, "A", true), Duration::from_secs(5));
    finish(&fake(&slot), &id, &one.url, false);
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::UNAVAILABLE);
    // The panel is closed while the page is being asked.
    let (two, _root) = app_in(&f, 2);
    let rx = ask(&f, app_op(&id, &two, "B", true), Duration::from_secs(5));
    finish(&fake(&slot), &id, &two.url, true);
    f.pump();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    let closed = ask(
        &f,
        CtlOp::PanelClose { tab: id.clone() },
        Duration::from_secs(5),
    );
    closed.try_recv().unwrap().unwrap();
    f.h.advance(0);
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::UNAVAILABLE);
    // The page moves on while the question is out.
    let (three, _root) = app_in(&f, 3);
    let rx = ask(&f, app_op(&id, &three, "C", true), Duration::from_secs(5));
    finish(&fake(&slot), &id, &three.url, true);
    f.pump();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    fake(&slot).say(WebEvent::Loading {
        tab: id.clone(),
        url: three.url.clone(),
        nav: 777,
    });
    f.pump();
    assert_eq!(rx.try_recv().unwrap().unwrap_err().code, code::UNAVAILABLE);
}

#[test]
fn the_agents_browser_calls_work_on_a_mini_app() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = f.ctl.borrow().front_id().unwrap();
    let (spec, _root) = app_in(&f, 1);
    shown_app(&f, &slot, &id, &spec, "Chart");
    with_shop(&slot);
    let rx = look(&f, &id);
    f.pump();
    let v = rx.try_recv().unwrap().unwrap();
    assert_eq!(v["elements"].as_array().unwrap().len(), 2);
}

#[test]
fn a_mini_app_is_never_stored_and_a_stored_app_address_is_dropped_on_restore() {
    // An app's panel is not written to panels.json ...
    let f = fx_with(Opts {
        saved: vec![("C:\\a", None)],
        ..Opts::default()
    });
    let file = f.dir.path().join("panels.json");
    f.ctl
        .borrow_mut()
        .set_panel_store(Arc::new(flare_surfaces::store::PanelStore::new(
            &file,
            Duration::from_millis(30),
        )));
    let slot = with_fake(&f);
    f.start_restore_and_wait();
    f.wait_made("C:\\a");
    let id = f.ctl.borrow().front_id().unwrap();
    let (spec, _root) = app_in(&f, 1);
    shown_app(&f, &slot, &id, &spec, "Chart");
    f.ctl.borrow_mut().refresh();
    std::thread::sleep(Duration::from_millis(250)); // sync-ok: test code only, past the store's 30 ms delay
    assert!(
        !file.exists()
            || !std::fs::read_to_string(&file) // sync-ok: test code only
                .unwrap()
                .contains("flare-app-"),
        "no app address on disk"
    );
    // ... and one an old or edited file holds is dropped, like any address a panel cannot open.
    let g = fx_with(Opts {
        saved: vec![("C:\\a", None)],
        ..Opts::default()
    });
    let file = g.dir.path().join("panels.json");
    let raw = r#"{"version":1,"tabs":[
        {"cwd":"C:\\a","panel":{"kind":"web","target":"https://flare-app-1.local/index.html"}}]}"#;
    std::fs::write(&file, raw).unwrap(); // sync-ok: test code only
    g.ctl
        .borrow_mut()
        .set_panel_store(Arc::new(flare_surfaces::store::PanelStore::new(
            &file,
            Duration::from_millis(30),
        )));
    let slot = with_fake(&g);
    g.start_restore_and_wait();
    g.h.advance(0);
    g.pump();
    assert_eq!(g.ctl.borrow().open_panels(), 0);
    assert!(slot.borrow().is_none(), "no browser for a dead app");
}

#[test]
fn a_shell_that_exits_takes_its_web_panel_and_browser_with_it() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let b = f.ctl.borrow_mut().open_folder(r"C:\b");
    f.wait_made(r"C:\b");
    f.pump();
    assert_eq!(f.ctl.borrow().front_id().as_deref(), Some(b.as_str()));
    let id = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    assert_eq!(id, b);
    assert_eq!(f.ctl.borrow().web_controllers(), 1);
    // The person types `exit`: the tab goes unasked, and so must everything held for it.
    f.made_with(r"C:\b")[0].exit(0);
    f.pump_until("the tab to close", |f| f.rows() == 1);
    f.h.advance(0);
    assert_eq!(f.ctl.borrow().open_panels(), 0, "the panel entry is gone");
    assert_eq!(f.ctl.borrow().web_controllers(), 0, "so is its browser");
    assert!(fake(&slot).calls().contains(&Call::Close(b)));
}

/// The last rectangle the fake was told for the tab.
fn last_bounds(slot: &Slot, tab: &str) -> Option<PhysRect> {
    fake(slot).calls().into_iter().rev().find_map(|c| match c {
        Call::Bounds(t, r) if t == tab => Some(r),
        _ => None,
    })
}

/// The window's client area, as a rectangle at its origin.
fn whole_window(f: &Fx) -> PhysRect {
    let size = slint::ComponentHandle::window(&f.h.comp).size();
    PhysRect {
        x: 0,
        y: 0,
        w: size.width as i32,
        h: size.height as i32,
    }
}

fn full_screen(f: &Fx, slot: &Slot, tab: &str, on: bool) {
    fake(slot).say(WebEvent::FullScreen {
        tab: tab.into(),
        on,
    });
    f.pump();
    f.h.advance(0);
    f.pump();
}

/// The owner (2026-10-24): a video's full-screen button did nothing, because the panel ignored the page's ask.
#[test]
fn a_page_in_full_screen_covers_the_window_and_gives_it_back_when_it_leaves() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    let slot_rect = drawn(&f);
    assert!(!f.h.comp.window().is_fullscreen());

    full_screen(&f, &slot, &id, true);
    assert!(f.ctl.borrow().web.covers_window());
    assert!(
        f.h.comp.window().is_fullscreen(),
        "the window goes full screen"
    );
    assert_eq!(
        last_bounds(&slot, &id),
        Some(whole_window(&f)),
        "the page covers the window"
    );

    full_screen(&f, &slot, &id, false);
    assert!(!f.ctl.borrow().web.covers_window());
    assert!(
        !f.h.comp.window().is_fullscreen(),
        "the window is as it was"
    );
    // The window kept its size for full screen, so nothing waits for it to shrink: the page is back at once.
    assert_eq!(last_bounds(&slot, &id), Some(drawn(&f)), "back on the slot");
    assert_eq!(drawn(&f), slot_rect);
}

/// The owner (2026-10-24): leaving full screen shook the UI, the page first moving into a panel laid out for the
/// full-screen window and then again once the window had shrunk.
#[test]
fn a_page_leaving_full_screen_moves_into_the_panel_once_the_window_has_its_size() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    let size = f.h.comp.window().size();
    let slot_rect = drawn(&f);

    full_screen(&f, &slot, &id, true);
    // The OS gives the full-screen window the whole monitor.
    f.h.comp.window().set_size(slint::PhysicalSize::new(
        size.width + 400,
        size.height + 200,
    ));
    f.ctl.borrow_mut().refresh();
    f.h.advance(0);
    f.pump();
    let big = whole_window(&f);
    assert_eq!(
        last_bounds(&slot, &id),
        Some(big),
        "the page covers the bigger window"
    );

    full_screen(&f, &slot, &id, false);
    assert!(
        !f.h.comp.window().is_fullscreen(),
        "the window is leaving full screen"
    );
    assert_eq!(
        last_bounds(&slot, &id),
        Some(big),
        "until the window shrinks, the page keeps covering it"
    );

    f.h.comp.window().set_size(size);
    f.ctl.borrow_mut().refresh();
    f.h.advance(0);
    f.pump();
    assert_eq!(
        last_bounds(&slot, &id),
        Some(slot_rect),
        "then straight onto the slot"
    );
}

fn asked_to_leave_full_screen(slot: &Slot, tab: &str) -> bool {
    fake(slot)
        .calls()
        .iter()
        .any(|c| matches!(c, Call::Script(t, _, js) if t == tab && js.contains("exitFullscreen")))
}

/// The review (2026-10-24): a menu over a page in full screen had a whole-window frame squashed into the panel.
#[test]
fn a_flare_menu_over_a_page_in_full_screen_ends_the_full_screen() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    full_screen(&f, &slot, &id, true);
    assert!(f.h.comp.window().is_fullscreen());
    f.h.comp.set_dialog_open(true);
    f.ctl.borrow_mut().refresh();
    f.h.advance(0);
    f.pump();
    assert!(!f.ctl.borrow().web.covers_window());
    assert!(
        asked_to_leave_full_screen(&slot, &id),
        "the page is told to leave full screen"
    );
    assert!(
        !f.h.comp.window().is_fullscreen(),
        "the window is as it was"
    );
    assert_eq!(
        last_bounds(&slot, &id),
        Some(drawn(&f)),
        "the page is back in its panel"
    );
    // The dialog closes: the page stays in its panel.
    f.h.comp.set_dialog_open(false);
    f.ctl.borrow_mut().refresh();
    f.h.advance(0);
    f.pump();
    assert!(!f.h.comp.window().is_fullscreen());
}

/// The review (2026-10-24): F11 during a page in full screen left the page covering a window that was not.
#[test]
fn f11_takes_a_page_out_of_full_screen_and_the_window_back() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    full_screen(&f, &slot, &id, true);
    assert!(f.h.comp.window().is_fullscreen());
    // F11 acts, and the shell hears it too.
    f.key(KeyEvent::new("F11", "F11"));
    f.h.advance(0);
    f.pump();
    assert!(!f.ctl.borrow().web.covers_window());
    assert!(asked_to_leave_full_screen(&slot, &id));
    assert!(
        !f.h.comp.window().is_fullscreen(),
        "the window is as it was"
    );
    assert_eq!(
        last_bounds(&slot, &id),
        Some(drawn(&f)),
        "the page is back in its panel"
    );
    // The page's own word that it has left changes nothing, and the next F11 is the window's again.
    full_screen(&f, &slot, &id, false);
    assert!(!f.h.comp.window().is_fullscreen());
    // F11 acts, and the shell hears it too.
    f.key(KeyEvent::new("F11", "F11"));
    f.h.advance(0);
    assert!(
        f.h.comp.window().is_fullscreen(),
        "F11 makes the window full screen"
    );
}

#[test]
fn a_page_in_full_screen_gives_the_window_back_while_its_tab_is_behind() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let a = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    full_screen(&f, &slot, &a, true);
    assert!(f.h.comp.window().is_fullscreen());
    let _b = f.ctl.borrow_mut().open_folder(r"C:\b");
    f.ctl.borrow_mut().refresh();
    f.h.advance(0);
    f.pump();
    assert!(
        !f.h.comp.window().is_fullscreen(),
        "another tab in front: the window is back"
    );
    // Back on the first tab, the page is still in full screen and covers the window again.
    assert_eq!(
        f.key(KeyEvent::new("1", "Digit1").ctrl()),
        KeyOutcome::Consumed
    );
    f.h.advance(0);
    f.pump();
    assert!(f.h.comp.window().is_fullscreen());
}

#[test]
fn a_window_already_full_screen_stays_so_when_the_page_leaves_full_screen() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    f.h.comp.window().set_fullscreen(true);
    full_screen(&f, &slot, &id, true);
    full_screen(&f, &slot, &id, false);
    assert!(
        f.h.comp.window().is_fullscreen(),
        "the person's own full screen is kept"
    );
}

/// macOS (CEF, port phase 5): a backend that draws off screen. Its picture is shown in the page box, the pointer, the
/// wheel and the keys over the box go to it (in device pixels, the key with its virtual key, its Mac key code and its
/// text), and a press or a typed key while an agent works in the page is the person taking it over. A backend with
/// no picture (WebView2) leaves the box empty and takes no input there.
#[test]
fn an_off_screen_page_is_drawn_in_its_box_takes_the_input_there_and_a_press_stops_the_agent() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    assert!(
        !f.h.comp.get_panel_web_live(),
        "no picture yet: WebView2's case"
    );
    fake(&slot).state.borrow_mut().frame = Some(flare_web::ViewFrame {
        width: 4,
        height: 2,
        bgra: std::sync::Arc::new([0xcc, 0x66, 0x33, 0xff].repeat(8)),
        serial: 1,
    });
    fake(&slot).say(WebEvent::Pump);
    f.pump();
    assert!(f.h.comp.get_panel_web_live(), "the picture is shown");
    assert_eq!(f.h.comp.get_panel_web_frame().size().width, 4);
    let scale = f.h.comp.get_scale();
    // Slint calls Cmd `control` on a Mac (rule 1110): the panel's bit 2 there, 8 elsewhere.
    let cmd_bit = if cfg!(target_vendor = "apple") { 2 } else { 8 };
    f.h.comp.invoke_panel_web_pointer(0, 10.0, 20.0, 0, cmd_bit);
    f.h.comp.invoke_panel_web_pointer(1, 10.0, 20.0, 0, 0);
    f.h.comp.invoke_panel_web_wheel(10.0, 20.0, 0.0, -3.0, 0);
    f.h.comp.invoke_panel_web_key(true, "a".into(), 0, false);
    f.h.comp
        .invoke_panel_web_key(true, "\u{10}".into(), 1, false);
    let calls = fake(&slot).calls();
    let page = flare_web::Slot::panel(&id);
    let pointers: Vec<flare_web::Pointer> = calls
        .iter()
        .filter_map(|c| match c {
            Call::Pointer(s, p) if *s == page => Some(*p),
            _ => None,
        })
        .collect();
    assert_eq!(pointers.len(), 3, "{calls:?}");
    assert_eq!(
        pointers[0].act,
        flare_web::PointerAct::Down(flare_web::PointerButton::Left, 1)
    );
    assert!(pointers[0].mods.cmd);
    assert!(
        (pointers[0].x - 10.0 * scale).abs() < 0.01 && (pointers[0].y - 20.0 * scale).abs() < 0.01
    );
    assert_eq!(
        pointers[2].act,
        flare_web::PointerAct::Wheel {
            dx: 0.0,
            dy: 3.0 * scale
        },
        "a wheel turned toward the person scrolls the page down"
    );
    assert!(
        calls.contains(&Call::Focus(id.clone())),
        "a press gives the page the keys"
    );
    let keys: Vec<&flare_web::KeyInput> = calls
        .iter()
        .filter_map(|c| match c {
            Call::Key(s, k) if *s == page => Some(k),
            _ => None,
        })
        .collect();
    assert_eq!(keys.len(), 1, "a modifier alone goes nowhere: {keys:?}");
    assert_eq!(
        (keys[0].vk, keys[0].mac_code, keys[0].text.as_str()),
        (0x41, 0x00, "a")
    );
    // While the agent works, the person's press ends its call.
    let rx = look(&f, &id);
    f.pump();
    assert_eq!(f.ctl.borrow().web.agent_op(&id), Some("look"));
    f.h.comp.invoke_panel_web_pointer(0, 30.0, 30.0, 0, 0);
    f.pump();
    assert!(rx.try_recv().unwrap().is_err(), "the call ended");
    assert_eq!(f.ctl.borrow().web.agent_op(&id), None);
    // A shortcut (Cmd with a key) is not the person taking the page; a typed key is.
    let rx = look(&f, &id);
    f.pump();
    f.h.comp
        .invoke_panel_web_key(true, "c".into(), cmd_bit, false);
    f.pump();
    assert_eq!(
        f.ctl.borrow().web.agent_op(&id),
        Some("look"),
        "Cmd+C leaves the agent's call running"
    );
    f.h.comp.invoke_panel_web_key(true, "x".into(), 0, false);
    f.pump();
    assert!(
        rx.try_recv().unwrap().is_err(),
        "a typed key ended the call"
    );
    assert_eq!(f.ctl.borrow().web.agent_op(&id), None);
    // The picture goes when the page has none.
    fake(&slot).state.borrow_mut().frame = None;
    fake(&slot).say(WebEvent::Pump);
    f.pump();
    assert!(!f.h.comp.get_panel_web_live());
}

// ---- pop-ups (rules 249, 250) ----

/// The page of the tab opens a pop-up from `site` and the window takes the news in.
fn popup(f: &Fx, slot: &Slot, tab: &str, site: &str) {
    fake(slot).open_popup(tab, site);
    f.pump();
    f.h.advance(0);
    f.pump();
}

/// How many times the page was sent somewhere or stepped through its history.
fn moves(slot: &Slot) -> usize {
    fake(slot)
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::Navigate(..) | Call::Go(..)))
        .count()
}

#[test]
fn the_bar_leaves_the_page_behind_a_popup_alone() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    fake(&slot).say(WebEvent::History {
        tab: id.clone(),
        can_back: true,
        can_forward: true,
    });
    f.pump();
    f.h.advance(0);
    popup(&f, &slot, &id, "bank.example");
    let moved = moves(&slot);
    f.h.comp.invoke_panel_web_back();
    f.h.comp.invoke_panel_web_forward();
    f.h.comp.invoke_panel_web_reload();
    f.h.comp.invoke_panel_web_stop();
    f.h.comp
        .invoke_panel_web_submit("example.com/elsewhere".into());
    f.pump();
    f.h.advance(0);
    assert_eq!(
        moves(&slot),
        moved,
        "the page the pop-up links back to stays as it is"
    );
    assert_eq!(f.ctl.borrow().panels.web_target(&id).as_deref(), Some(PAGE));
    assert_eq!(
        f.h.comp.get_panel_sub().as_str(),
        crate::web_ctl::POPUP_FIRST
    );
    // Once the person closes it, the note goes and the bar drives the page again.
    f.h.comp.invoke_panel_web_popup_close();
    f.pump();
    f.h.advance(0);
    f.pump();
    assert_eq!(f.h.comp.get_panel_sub().as_str(), "");
    f.h.comp.invoke_panel_web_reload();
    assert!(
        fake(&slot)
            .calls()
            .contains(&Call::Go(id.clone(), Nav::Reload))
    );
}

#[test]
fn a_popup_shows_under_flares_strip_and_closing_it_brings_the_page_back() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    f.pump();
    let page = drawn(&f);
    assert_eq!(f.h.comp.get_panel_web_popup().as_str(), "");
    assert_eq!(f.h.comp.get_panel_popup_h(), 0.0);
    popup(&f, &slot, &id, "bank.example");
    assert_eq!(
        f.h.comp.get_panel_web_popup().as_str(),
        "Pop-up from bank.example",
        "the strip names the site, never the page's title"
    );
    let strip = f.h.comp.get_panel_popup_h();
    assert!(strip > 0.0, "the strip takes room");
    let under = drawn(&f);
    assert!(under.y > page.y && under.h < page.h, "{page:?} {under:?}");
    assert_eq!(
        last_bounds(&slot, &id),
        Some(under),
        "what shows in the panel sits under the strip"
    );
    // A second pop-up from the pop-up stacks on top: the strip names the top one.
    popup(&f, &slot, &id, "acs.example");
    assert_eq!(
        f.h.comp.get_panel_web_popup().as_str(),
        "Pop-up from acs.example"
    );
    // Close takes the top one away; the one below shows again, then the page.
    f.h.comp.invoke_panel_web_popup_close();
    f.pump();
    f.h.advance(0);
    assert_eq!(
        f.h.comp.get_panel_web_popup().as_str(),
        "Pop-up from bank.example"
    );
    f.h.comp.invoke_panel_web_popup_close();
    f.pump();
    f.h.advance(0);
    f.pump();
    assert_eq!(f.h.comp.get_panel_web_popup().as_str(), "");
    assert_eq!(f.h.comp.get_panel_popup_h(), 0.0);
    assert_eq!(drawn(&f), page, "the page has its whole box again");
    assert_eq!(last_bounds(&slot, &id), Some(page));
    let closes = fake(&slot)
        .calls()
        .iter()
        .filter(|c| **c == Call::ClosePopup(id.clone()))
        .count();
    assert_eq!(closes, 2);
    assert!(fake(&slot).popups(&id).is_empty());
}

#[test]
fn the_agents_input_calls_wait_for_the_person_while_a_popup_is_open() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    with_shop(&slot);
    popup(&f, &slot, &id, "bank.example");
    let refused = |rx: Receiver<Result<Value, flare_ctl::CtlError>>, what: &str| {
        f.pump();
        let e = rx.try_recv().expect("answered at once").expect_err(what);
        assert_eq!(e.code, code::UNAVAILABLE, "{what}");
        assert_eq!(e.message, crate::web_ctl::POPUP_OPEN, "{what}");
    };
    let before = cdp_calls(&slot).len();
    let moved = moves(&slot);
    refused(click_ref(&f, &id, "e1"), "a click");
    for (op, what) in [
        (
            AgentOp::Type {
                text: "4242".into(),
                target: None,
            },
            "typing",
        ),
        (
            AgentOp::Scroll {
                dx: 0.0,
                dy: 100.0,
                target: None,
            },
            "a scroll",
        ),
        (
            AgentOp::Eval {
                expression: "1".into(),
            },
            "eval",
        ),
    ] {
        refused(ask(&f, agent_op(&id, op), Duration::from_secs(5)), what);
    }
    refused(
        ask(
            &f,
            CtlOp::BrowserNavigate {
                tab: id.clone(),
                to: NavTo::Url(TWO.into()),
            },
            Duration::from_secs(5),
        ),
        "a navigation",
    );
    // open_page on the tab would send the page behind the pop-up away and break its link back.
    refused(
        ask(&f, open_op(&id, TWO), Duration::from_secs(5)),
        "open_page",
    );
    assert_eq!(
        cdp_calls(&slot).len(),
        before,
        "nothing reached the page behind the pop-up"
    );
    assert_eq!(
        moves(&slot),
        moved,
        "the page behind the pop-up was not sent anywhere"
    );
    assert_eq!(f.ctl.borrow().panels.web_target(&id).as_deref(), Some(PAGE));
    // Reading the page behind it still works, and the panel's state says a pop-up is open.
    let rx = look(&f, &id);
    f.pump();
    assert!(
        rx.try_recv().unwrap().is_ok(),
        "a look still reads the page"
    );
    let v = ask(
        &f,
        CtlOp::PanelState {
            tab: id.clone(),
            cwd: None,
        },
        Duration::from_secs(5),
    )
    .try_recv()
    .unwrap()
    .unwrap();
    assert_eq!(v["popup"], json!(true));
    // The person finishes it: the agent may act again.
    f.h.comp.invoke_panel_web_popup_close();
    f.pump();
    let v = ask(
        &f,
        CtlOp::PanelState {
            tab: id.clone(),
            cwd: None,
        },
        Duration::from_secs(5),
    )
    .try_recv()
    .unwrap()
    .unwrap();
    assert!(v.get("popup").is_none(), "{v}");
    let rx = click_ref(&f, &id, "e1");
    f.pump();
    agent_turn(&f, GLIDE_MAX_MS);
    assert!(
        rx.try_recv().unwrap().is_ok(),
        "the click goes through again"
    );
}

#[test]
fn closing_the_tab_takes_its_popups_with_it() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let a = up(&f, &slot);
    popup(&f, &slot, &a, "bank.example");
    assert!(f.ctl.borrow().web.popup_open(&a));
    let _b = f.ctl.borrow_mut().open_folder("C:\\b");
    f.ctl.borrow_mut().request_close(&a);
    f.pump();
    f.h.advance(0);
    assert!(fake(&slot).calls().contains(&Call::Close(a.clone())));
    assert!(
        fake(&slot).popups(&a).is_empty(),
        "the pop-ups went with the page"
    );
    assert!(!f.ctl.borrow().web.popup_open(&a));
    assert_eq!(
        f.h.comp.get_panel_web_popup().as_str(),
        "",
        "no strip is left for the next tab"
    );
}

#[test]
fn a_popup_no_click_asked_for_is_said_in_the_bar() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    f.h.advance(0);
    fake(&slot).say(WebEvent::Note {
        tab: id.clone(),
        note: flare_web::WebNote::new(flare_web::NoteKind::Popup, "https://ads.example/pop", PAGE)
            .detail("blocked")
            .user(false),
    });
    f.pump();
    f.h.advance(0);
    assert_eq!(f.h.comp.get_panel_sub().as_str(), "Pop-up blocked");
    assert_eq!(
        f.h.comp.get_panel_web_popup().as_str(),
        "",
        "nothing opened"
    );
    assert_eq!(
        f.ctl.borrow().web.tab_url(&id),
        Some(PAGE),
        "the page stays where it is"
    );
}

// ---- app links (rule 252) ----

/// The recording launcher the window opens app links with; nothing real is ever opened.
fn launcher(f: &Fx) -> Rc<crate::app_ask::RecordingLauncher> {
    let l = Rc::new(crate::app_ask::RecordingLauncher::default());
    f.ctl.borrow_mut().set_app_launcher(l.clone());
    l
}

fn app_link(fake: &FakeWeb, tab: &str, uri: &str, site: &str) {
    let scheme = uri.split(':').next().unwrap_or_default().to_string();
    fake.say(WebEvent::AppLink {
        tab: tab.into(),
        uri: uri.into(),
        scheme,
        site: site.into(),
    });
}

const BANKID: &str = "bankid:///?autostarttoken=abc&redirect=null";

#[test]
fn an_app_link_is_asked_about_with_cancel_the_default_and_opened_only_on_open() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let apps = launcher(&f);
    f.clock.set(10_000);
    app_link(&fake(&slot), &id, BANKID, "bank.example");
    f.pump();
    assert!(f.h.comp.get_dialog_open());
    assert_eq!(f.h.comp.get_dialog_title().as_str(), "Open an app?");
    assert_eq!(f.h.comp.get_dialog_primary().as_str(), "Open");
    assert!(
        f.h.comp.get_dialog_default_cancel(),
        "Cancel is the default"
    );
    assert!(apps.opened().is_empty(), "nothing opens before the answer");
    // A click on Open in the first 0.3 s does nothing; the question stays.
    f.clock.set(10_299);
    f.h.comp.invoke_dialog_confirm();
    assert!(apps.opened().is_empty());
    assert!(f.h.comp.get_dialog_open());
    f.clock.set(10_300);
    f.h.comp.invoke_dialog_confirm();
    assert_eq!(apps.opened(), vec![BANKID.to_string()]);
    assert!(!f.h.comp.get_dialog_open());
    assert!(
        !f.h.comp.get_dialog_default_cancel(),
        "the next question has its own default"
    );
}

#[test]
fn a_cancelled_app_link_is_not_asked_again_until_the_page_changes_site() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let apps = launcher(&f);
    app_link(&fake(&slot), &id, BANKID, "flare-test.local");
    f.pump();
    f.h.comp.invoke_dialog_cancel();
    assert!(!f.h.comp.get_dialog_open());
    // Another page of the same site keeps the Cancel.
    fake(&slot).say(WebEvent::Url {
        tab: id.clone(),
        url: TWO.into(),
    });
    f.pump();
    f.clock.set(60_000);
    app_link(&fake(&slot), &id, BANKID, "flare-test.local");
    f.pump();
    f.h.advance(0);
    assert!(!f.h.comp.get_dialog_open(), "muted");
    assert_eq!(
        f.h.comp.get_panel_sub().as_str(),
        crate::app_ask::APP_LINK_HELD
    );
    // The page goes to another site and back: the person is asked again.
    fake(&slot).say(WebEvent::Url {
        tab: id.clone(),
        url: "https://other.example/".into(),
    });
    f.pump();
    app_link(&fake(&slot), &id, BANKID, "flare-test.local");
    f.pump();
    assert!(f.h.comp.get_dialog_open());
    assert!(apps.opened().is_empty());
}

#[test]
fn an_app_nothing_on_the_pc_opens_is_said_in_the_bar_and_not_asked_about() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let apps = launcher(&f);
    apps.missing.lock().unwrap().push("swish".into());
    app_link(
        &fake(&slot),
        &id,
        "swish://paymentrequest?token=x",
        "shop.example",
    );
    f.pump();
    f.h.advance(0);
    assert!(!f.h.comp.get_dialog_open());
    assert_eq!(
        f.h.comp.get_panel_sub().as_str(),
        format!("Nothing on this {} opens swish:", crate::app_ask::MACHINE)
    );
    assert!(apps.opened().is_empty());
}

#[test]
fn an_app_link_of_a_closed_page_is_not_asked_about() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let apps = launcher(&f);
    app_link(&fake(&slot), &id, BANKID, "bank.example");
    f.pump();
    assert!(f.h.comp.get_dialog_open());
    assert_eq!(f.key(shift_ctrl("P", "KeyP")), KeyOutcome::Consumed);
    f.h.advance(0);
    f.pump();
    assert!(
        !f.h.comp.get_dialog_open(),
        "the question went with the page"
    );
    f.clock.set(1_000);
    f.h.comp.invoke_dialog_confirm();
    assert!(apps.opened().is_empty());
}

#[test]
fn a_download_goes_first_and_the_app_link_is_asked_after_it() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let apps = launcher(&f);
    download(&fake(&slot), &id, 1, "a.zip");
    app_link(&fake(&slot), &id, BANKID, "bank.example");
    f.pump();
    assert_eq!(f.h.comp.get_dialog_title().as_str(), "Save this file?");
    assert!(!f.h.comp.get_dialog_default_cancel());
    f.clock.set(5_000);
    f.h.comp.invoke_dialog_cancel();
    assert_eq!(answers(&slot), vec![(1, false)]);
    assert_eq!(f.h.comp.get_dialog_title().as_str(), "Open an app?");
    assert!(f.h.comp.get_dialog_default_cancel());
    // Its guard starts when it shows, not when it arrived.
    f.clock.set(5_100);
    f.h.comp.invoke_dialog_confirm();
    assert!(apps.opened().is_empty());
    f.clock.set(5_300);
    f.h.comp.invoke_dialog_confirm();
    assert_eq!(apps.opened(), vec![BANKID.to_string()]);
}

/// Two tabs' app links: the second question takes the open dialog with the same title and default, and still starts on
/// Cancel, so the Tab the person gave the first never opens the second with a plain Enter (review of 2026-11-10).
#[test]
fn the_next_app_link_question_starts_on_cancel_after_a_tab_to_open_the_first() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let first = up(&f, &slot);
    assert_eq!(f.key(ctrl("t", "KeyT")), KeyOutcome::Consumed);
    f.h.advance(0);
    f.pump();
    let second = up(&f, &slot);
    assert_ne!(first, second);
    let apps = launcher(&f);
    f.clock.set(10_000);
    app_link(&fake(&slot), &first, BANKID, "bank.example");
    app_link(&fake(&slot), &second, BANKID, "other.example");
    f.pump();
    f.h.advance(0);
    assert!(f.h.comp.get_dialog_open());
    assert_eq!(f.h.comp.get_dialog_focus(), 0, "Cancel has the focus");
    f.clock.set(10_400);
    f.h.key_text("\t\n");
    assert_eq!(
        apps.opened(),
        vec![BANKID.to_string()],
        "Tab then Enter opens"
    );
    f.h.advance(0);
    assert!(f.h.comp.get_dialog_open(), "the second question is up");
    assert_eq!(f.h.comp.get_dialog_title().as_str(), "Open an app?");
    assert_eq!(
        f.h.comp.get_dialog_focus(),
        0,
        "the second question starts on Cancel"
    );
    f.clock.set(11_000);
    f.h.key_text("\n");
    assert_eq!(apps.opened().len(), 1, "a plain Enter cancels it");
    assert!(!f.h.comp.get_dialog_open());
}

/// The shell answers from its worker (rule 34): a failure reaches the bar once that answer is handled on the UI thread.
#[test]
fn an_app_that_could_not_be_opened_is_said_in_the_bar_when_its_worker_answers() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let apps = launcher(&f);
    *apps.fails.lock().unwrap() = true;
    f.clock.set(10_000);
    app_link(&fake(&slot), &id, BANKID, "bank.example");
    f.pump();
    f.clock.set(10_300);
    f.h.comp.invoke_dialog_confirm();
    assert_eq!(apps.opened(), vec![BANKID.to_string()]);
    assert!(!f.h.comp.get_dialog_open());
    f.pump();
    f.h.advance(0);
    assert_eq!(
        f.h.comp.get_panel_sub().as_str(),
        crate::app_ask::APP_NOT_OPENED
    );
}

#[test]
fn while_an_agent_waits_on_the_page_an_app_link_is_never_asked_and_its_call_answered_once() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let apps = launcher(&f);
    // The agent sent the page somewhere and waits for it to load.
    let rx = ask(&f, open_op(&id, TWO), Duration::from_secs(5));
    f.pump();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    app_link(&fake(&slot), &id, BANKID, "bank.example");
    f.pump();
    f.h.advance(0);
    assert!(
        !f.h.comp.get_dialog_open(),
        "no question while an agent drives"
    );
    let e = rx.try_recv().unwrap().unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    assert_eq!(e.message, crate::app_ask::APP_LINK_AGENT_ANSWER);
    assert_eq!(
        f.h.comp.get_panel_sub().as_str(),
        crate::app_ask::APP_LINK_AGENT
    );
    // The call is over: a link after it is the person's to answer, and the call hears nothing more.
    app_link(&fake(&slot), &id, BANKID, "bank.example");
    f.pump();
    assert!(rx.try_recv().is_err(), "answered exactly once");
    assert!(f.h.comp.get_dialog_open());
    assert!(apps.opened().is_empty());
}

#[test]
fn an_agent_call_running_on_the_page_is_answered_that_only_the_person_can_open_an_app() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let apps = launcher(&f);
    // A wait whose page never answers: the call is running.
    let rx = ask(
        &f,
        agent_op(
            &id,
            AgentOp::Wait {
                what: WaitFor::Text("Paid".into()),
                timeout_ms: 30_000,
            },
        ),
        Duration::from_secs(5),
    );
    f.pump();
    assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    app_link(&fake(&slot), &id, BANKID, "bank.example");
    f.pump();
    assert!(!f.h.comp.get_dialog_open());
    let e = rx.try_recv().unwrap().unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    assert_eq!(e.message, crate::app_ask::APP_LINK_AGENT_ANSWER);
    // The hold after the call still counts as the agent's: a second link is not asked about, and nothing answers twice.
    app_link(&fake(&slot), &id, BANKID, "bank.example");
    f.pump();
    assert!(!f.h.comp.get_dialog_open());
    assert!(rx.try_recv().is_err());
    assert!(apps.opened().is_empty());
}
