//! Arrows over the page (2026-11-10, rules 254 to 256) inside the window controller, with a stand-in browser, the
//! recording windows and a stand-in mouse: the agent's arrow (`browser.point`) and every way it ends or hides, and the
//! person's arrow drawn with "Draw arrow", every way the draw mode is cancelled, the frame typed unsent and the arrow
//! the agent's next look gets.

use std::cell::Cell;
use std::sync::mpsc::Receiver;

use flare_ctl::code;
use flare_surfaces::agent_job::{Op as AgentOp, Target};
use flare_surfaces::web::{PhysRect, phys_rect};
use flare_web::fake::{Call, FakeWeb};
use flare_web::{KeyPress, WebEvent};
use flare_win::agent_cursor::{recorded_arrow, recorded_mark, recorded_pad};
use serde_json::{Value, json};

use super::*;
use crate::ctl_app::{CtlOp, CtlRequest, CtlRequestBox};
use crate::web_ctl::arrows::{POINT_MEASURE_MS, POINT_MS, PointerNow, USER_ARROW_MS};
use crate::web_ctl::{PointReq, WebMsg, WebMsgBox};

const PAGE: &str = "https://flare-test.local/index.html";
const TWO: &str = "https://flare-test.local/two.html";

type Slot = Rc<RefCell<Option<FakeWeb>>>;
type Answer = Receiver<Result<Value, flare_ctl::CtlError>>;

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

fn ask(f: &Fx, op: CtlOp) -> Answer {
    let (tx, rx) = std::sync::mpsc::channel();
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Ctl(CtlRequestBox(CtlRequest {
            op,
            deadline: Instant::now() + Duration::from_secs(5),
            reply: tx,
        })));
    rx
}

/// The browser starts a navigation and finishes it.
fn finish(fake: &FakeWeb, tab: &str, url: &str) {
    thread_local! {
        static NAV: Cell<u64> = const { Cell::new(100) };
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

/// The front tab with its page up and loaded, and the page's place known.
fn up(f: &Fx, slot: &Slot) -> String {
    let id = f.ctl.borrow().front_id().unwrap();
    let rx = ask(
        f,
        CtlOp::BrowserOpen {
            tab: id.clone(),
            url: PAGE.into(),
        },
    );
    finish(&fake(slot), &id, PAGE);
    f.pump();
    assert!(rx.try_recv().unwrap().is_ok());
    f.h.advance(0);
    f.pump();
    id
}

/// A Claude tab in front.
fn claude(f: &Fx) -> String {
    let id = f.ctl.borrow().front_id().unwrap();
    let cwd = f.cwds()[0].clone();
    f.say(&cwd, TITLE_IDLE);
    f.pump_until("the agent to be here", |f| f.agent_here(&id));
    id
}

/// The shop page: a button (node 11) and a field (node 12) in a 600 x 400 viewport, moved up by `scrolled` CSS
/// pixels, and what is under a point (the field) and its accessibility node. A hidden element (node 99) has a place
/// off the page.
fn shop(scrolled: Rc<Cell<f64>>) -> flare_web::fake::CdpResponder {
    Box::new(move |method: &str, params: &str| {
        let p: Value = serde_json::from_str(params).unwrap_or(Value::Null);
        let dy = scrolled.get();
        let quad = |x: f64, y: f64, w: f64, h: f64| {
            json!({"quads": [[x, y - dy, x + w, y - dy, x + w, y + h - dy, x, y + h - dy]]})
                .to_string()
        };
        match method {
            "Page.getLayoutMetrics" => Ok(json!({"cssVisualViewport": {"pageX": 0, "pageY": dy, "clientWidth": 600, "clientHeight": 400}}).to_string()),
            "Accessibility.getFullAXTree" => Ok(json!({"nodes": [
                {"nodeId": "1", "role": {"value": "RootWebArea"}, "childIds": ["2", "3"], "backendDOMNodeId": 1},
                {"nodeId": "2", "role": {"value": "button"}, "name": {"value": "Buy"}, "backendDOMNodeId": 11},
                {"nodeId": "3", "role": {"value": "textbox"}, "name": {"value": "Quantity"}, "backendDOMNodeId": 12}
            ]}).to_string()),
            "Page.captureScreenshot" => Ok(json!({"data": "iVBORw0KGgoAAAANSUhEUgAAAyAAAAH0CAYAAADL1t+KAAAAAXNSR0IArs4c6QAAAA"}).to_string()),
            "DOM.getContentQuads" => match p["backendNodeId"].as_i64() {
                Some(11) => Ok(quad(100.0, 100.0, 80.0, 20.0)),
                Some(12) => Ok(quad(100.0, 150.0, 200.0, 20.0)),
                Some(99) => Ok(quad(100.0, 5000.0, 80.0, 20.0)),
                _ => Err("no box".into()),
            },
            "DOM.getNodeForLocation" => Ok(json!({"backendNodeId": 12, "nodeId": 0}).to_string()),
            "Accessibility.getPartialAXTree" => match p["backendNodeId"].as_i64() {
                Some(12) => Ok(json!({"nodes": [
                    {"nodeId": "3", "role": {"value": "textbox"}, "name": {"value": "Quantity\u{202e}"}, "backendDOMNodeId": 12}
                ]}).to_string()),
                _ => Err("no node".into()),
            },
            _ => Ok("{}".into()),
        }
    })
}

/// The shop answers the page's DevTools calls; the returned cell scrolls it.
fn with_shop(slot: &Slot) -> Rc<Cell<f64>> {
    let scrolled = Rc::new(Cell::new(0.0));
    fake(slot).state.borrow_mut().cdp = Some(shop(scrolled.clone()));
    scrolled
}

fn cdp_calls(slot: &Slot) -> Vec<(String, Value)> {
    fake(slot)
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Cdp(_, _, m, p) => Some((m, serde_json::from_str(&p).unwrap_or(Value::Null))),
            _ => None,
        })
        .collect()
}

fn look(f: &Fx, tab: &str) -> Value {
    let rx = ask(
        f,
        CtlOp::BrowserAgent {
            tab: tab.into(),
            op: AgentOp::Look { picture: true },
            cancel: flare_ctl::CancelFlag::default(),
        },
    );
    f.pump();
    rx.try_recv().expect("answered").expect("a look")
}

fn point(f: &Fx, tab: &str, target: Option<Target>, label: &str, clear: bool) -> Answer {
    let rx = ask(
        f,
        CtlOp::BrowserPoint {
            tab: tab.into(),
            req: PointReq {
                target,
                label: label.into(),
                clear,
            },
        },
    );
    f.pump();
    rx
}

fn point_ref(f: &Fx, tab: &str, r: &str) -> Value {
    point(f, tab, Some(Target::Ref(r.into())), "", false)
        .try_recv()
        .expect("answered at once")
        .expect("pointed")
}

/// The agent's clock moves on `ms` and its timer turns once.
fn agent_turn(f: &Fx, ms: u64) {
    f.ctl.borrow_mut().web.skip_clock(ms);
    f.ctl
        .borrow_mut()
        .on_msg(UiMsg::Web(WebMsgBox(WebMsg::AgentTick)));
    f.pump();
}

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

/// A CSS point of the shop's viewport in client pixels.
fn on_page(f: &Fx, x: f64, y: f64) -> (i32, i32) {
    let r = drawn(f);
    (
        r.x + (x * f64::from(r.w) / 600.0).round() as i32,
        r.y + (y * f64::from(r.h) / 400.0).round() as i32,
    )
}

/// Where the agent's arrow's head is drawn, in client pixels.
fn arrow_at(f: &Fx) -> Option<(i32, i32)> {
    let p = f.ctl.borrow().web.arrow_probe();
    let a = &p["agent"];
    a.as_array()
        .map(|a| (a[0].as_i64().unwrap() as i32, a[1].as_i64().unwrap() as i32))
}

fn head(f: &Fx, tab: &str) -> Option<[f64; 4]> {
    f.ctl.borrow().web.point_of(tab).and_then(|(h, _)| h)
}

fn has_point(f: &Fx, tab: &str) -> bool {
    f.ctl.borrow().web.point_of(tab).is_some()
}

/// A page with its look made, so its refs are known.
fn looked_shop(f: &Fx, slot: &Slot, tab: &str) -> Rc<Cell<f64>> {
    let scrolled = with_shop(slot);
    look(f, tab);
    scrolled
}

// ---- the agent's arrow (rule 254) ----

#[test]
fn the_agents_arrow_scrolls_its_element_into_view_answers_at_once_and_never_clicks() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    looked_shop(&f, &slot, &id);
    let before = cdp_calls(&slot).len();
    let rx = point(
        &f,
        &id,
        Some(Target::Ref("e1".into())),
        "  Buy it\nhere\u{200b}",
        false,
    );
    assert_eq!(
        rx.try_recv().unwrap().unwrap(),
        json!({"pointed": true, "offscreen": false})
    );
    let sent: Vec<(String, Value)> = cdp_calls(&slot).split_off(before);
    assert!(
        sent.iter()
            .any(|(m, p)| m == "DOM.scrollIntoViewIfNeeded" && p["backendNodeId"] == 11),
        "{sent:?}"
    );
    for (m, _) in &sent {
        assert!(
            [
                "DOM.enable",
                "DOM.scrollIntoViewIfNeeded",
                "Page.getLayoutMetrics",
                "DOM.getContentQuads"
            ]
            .contains(&m.as_str()),
            "nothing but where it is: {m}"
        );
    }
    let (at, label) = f.ctl.borrow().web.point_of(&id).unwrap();
    assert_eq!(at, Some([140.0, 110.0, 600.0, 400.0]));
    assert_eq!(label, "Buy it here", "cleaned to one line");
    assert_eq!(arrow_at(&f), Some(on_page(&f, 140.0, 110.0)));
    assert!(recorded_arrow().shown);
    // A ref the last look did not give is not found; a point off the top left is refused.
    let e = point(&f, &id, Some(Target::Ref("e9".into())), "", false)
        .try_recv()
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, code::NOT_FOUND);
    let e = point(&f, &id, Some(Target::Point(-1.0, 5.0)), "", false)
        .try_recv()
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, code::BAD_PARAMS);
    assert_eq!(
        head(&f, &id),
        Some([140.0, 110.0, 600.0, 400.0]),
        "the arrow up stays"
    );
}

#[test]
fn the_agents_arrow_follows_the_page_as_it_scrolls_and_hides_while_its_element_is_off_screen() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    let scrolled = looked_shop(&f, &slot, &id);
    point_ref(&f, &id, "e1");
    scrolled.set(50.0);
    agent_turn(&f, POINT_MEASURE_MS);
    assert_eq!(head(&f, &id), Some([140.0, 60.0, 600.0, 400.0]));
    assert_eq!(arrow_at(&f), Some(on_page(&f, 140.0, 60.0)));
    // Scrolled out of sight: the arrow is kept but not drawn, and comes back with its element.
    scrolled.set(1000.0);
    agent_turn(&f, POINT_MEASURE_MS);
    assert!(has_point(&f, &id) && head(&f, &id).is_none());
    assert_eq!(arrow_at(&f), None);
    assert!(!recorded_arrow().shown);
    scrolled.set(0.0);
    agent_turn(&f, POINT_MEASURE_MS);
    assert_eq!(arrow_at(&f), Some(on_page(&f, 140.0, 110.0)));
    // A point below what shows is pointed at, off screen.
    let v = point(&f, &id, Some(Target::Point(10.0, 900.0)), "", false)
        .try_recv()
        .unwrap()
        .unwrap();
    assert_eq!(v, json!({"pointed": true, "offscreen": true}));
}

#[test]
fn the_agents_arrow_goes_on_clear_a_new_arrow_and_after_a_minute() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    looked_shop(&f, &slot, &id);
    point_ref(&f, &id, "e1");
    // A new arrow replaces the last: one per tab.
    point_ref(&f, &id, "e2");
    assert_eq!(head(&f, &id), Some([200.0, 160.0, 600.0, 400.0]));
    assert_eq!(arrow_at(&f), Some(on_page(&f, 200.0, 160.0)));
    let v = point(&f, &id, None, "", true).try_recv().unwrap().unwrap();
    assert_eq!(v, json!({"cleared": true}));
    assert!(!has_point(&f, &id));
    assert_eq!(arrow_at(&f), None);
    assert!(!recorded_arrow().shown);
    let v = point(&f, &id, None, "", true).try_recv().unwrap().unwrap();
    assert_eq!(v, json!({"cleared": false}));
    // A minute on, it is gone by itself, and the timer has nothing left to watch for it.
    point_ref(&f, &id, "e1");
    agent_turn(&f, POINT_MS / 2);
    assert!(has_point(&f, &id));
    agent_turn(&f, POINT_MS / 2);
    assert!(!has_point(&f, &id));
    assert!(!recorded_arrow().shown);
}

#[test]
fn the_agents_arrow_goes_when_the_person_acts_the_page_moves_on_or_the_panel_closes() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    looked_shop(&f, &slot, &id);
    point_ref(&f, &id, "e1");
    f.ctl.borrow_mut().web_user_input();
    f.pump();
    assert!(!has_point(&f, &id), "the person's click or key");
    assert!(!recorded_arrow().shown);
    point_ref(&f, &id, "e1");
    finish(&fake(&slot), &id, TWO);
    f.pump();
    assert!(!has_point(&f, &id), "a navigation");
    assert!(!recorded_arrow().shown);
    looked_shop(&f, &slot, &id);
    point_ref(&f, &id, "e1");
    f.h.comp.invoke_panel_close();
    f.h.advance(0);
    f.pump();
    assert!(!has_point(&f, &id), "the panel closed");
    assert!(!recorded_arrow().shown);
}

#[test]
fn the_agents_arrow_hides_under_a_menu_another_tab_and_a_popup_and_is_refused_over_a_popup() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    looked_shop(&f, &slot, &id);
    point_ref(&f, &id, "e1");
    assert!(arrow_at(&f).is_some());
    // A Flare menu over the page: its still is drawn, and the arrow goes until it closes.
    f.h.comp.set_dialog_open(true);
    f.h.advance(0);
    f.pump();
    assert_eq!(arrow_at(&f), None);
    f.h.comp.set_dialog_open(false);
    f.h.advance(0);
    f.pump();
    assert_eq!(arrow_at(&f), Some(on_page(&f, 140.0, 110.0)));
    // A pop-up over the page: the arrow never draws over it, and no new one is taken (rule 250).
    fake(&slot).open_popup(&id, "bank.example");
    f.pump();
    f.h.advance(0);
    f.pump();
    assert_eq!(arrow_at(&f), None);
    assert!(!recorded_arrow().shown);
    let e = point(&f, &id, Some(Target::Ref("e2".into())), "", false)
        .try_recv()
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    assert_eq!(e.message, crate::web_ctl::POPUP_OPEN);
    f.h.comp.invoke_panel_web_popup_close();
    f.pump();
    f.h.advance(0);
    f.pump();
    assert!(arrow_at(&f).is_some(), "back once the pop-up is done");
    // Another tab in front: the arrow stays with its page and is not drawn.
    assert_eq!(f.key(ctrl("t", "KeyT")), KeyOutcome::Consumed);
    f.h.advance(0);
    f.pump();
    assert!(has_point(&f, &id));
    assert_eq!(arrow_at(&f), None);
}

// ---- the person's arrow (rules 255, 256) ----

/// The stand-in mouse: what the draw mode reads at its next look.
fn mouse(f: &Fx) -> Rc<Cell<PointerNow>> {
    let m = Rc::new(Cell::new(PointerNow {
        over_flare: true,
        ..PointerNow::default()
    }));
    let read = m.clone();
    f.ctl
        .borrow_mut()
        .web
        .set_pointer(Rc::new(move |_| Some(read.get())));
    m
}

fn at(m: &Rc<Cell<PointerNow>>, p: (i32, i32), left: bool) {
    m.set(PointerNow {
        x: p.0,
        y: p.1,
        left_down: left,
        left_pressed: left,
        right: false,
        over_flare: true,
    });
}

fn press_draw(f: &Fx) {
    f.h.comp.invoke_panel_draw_arrow();
    f.pump();
    f.h.advance(0);
    f.pump();
}

fn drawing(f: &Fx, tab: &str) -> bool {
    f.ctl.borrow().web.drawing(tab)
}

/// The person drags from `from` to `to` (CSS pixels of the shop) and lets go.
fn drag(f: &Fx, m: &Rc<Cell<PointerNow>>, from: (f64, f64), to: (f64, f64)) {
    at(m, on_page(f, from.0, from.1), false);
    agent_turn(f, 16);
    at(m, on_page(f, from.0, from.1), true);
    agent_turn(f, 16);
    at(
        m,
        on_page(f, (from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0),
        true,
    );
    agent_turn(f, 16);
    at(m, on_page(f, to.0, to.1), true);
    agent_turn(f, 16);
    at(m, on_page(f, to.0, to.1), false);
    agent_turn(f, 16);
}

fn asked_for_node(slot: &Slot) -> Vec<Value> {
    cdp_calls(slot)
        .into_iter()
        .filter(|(m, _)| m == "DOM.getNodeForLocation")
        .map(|(_, p)| p)
        .collect()
}

#[test]
fn the_draw_button_shows_for_an_agent_on_a_web_page_and_never_over_a_popup() {
    let (f, _h, _) = live(Opts::default());
    let slot = with_fake(&f);
    let id = up(&f, &slot);
    assert!(!f.h.comp.get_panel_draw_on(), "no agent in the tab");
    let tab = claude(&f);
    assert_eq!(tab, id);
    f.ctl.borrow_mut().refresh();
    f.pump();
    assert!(f.h.comp.get_panel_draw_on());
    assert!(f.h.comp.get_panel_draw_visible());
    assert!(!f.h.comp.get_panel_draw_active());
    fake(&slot).open_popup(&id, "bank.example");
    f.pump();
    f.h.advance(0);
    f.pump();
    assert!(!f.h.comp.get_panel_draw_on(), "not while a pop-up is open");
    f.h.comp.invoke_panel_web_popup_close();
    f.pump();
    f.h.advance(0);
    f.pump();
    assert!(f.h.comp.get_panel_draw_on());
}

#[test]
fn the_person_drags_an_arrow_and_its_frame_is_typed_unsent() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    let tab = claude(&f);
    let id = up(&f, &slot);
    assert_eq!(tab, id);
    with_shop(&slot);
    let m = mouse(&f);
    // The press on the button is still down when the mode starts: it is not a press on the page.
    at(&m, on_page(&f, 50.0, 50.0), true);
    press_draw(&f);
    assert!(drawing(&f, &id));
    assert!(f.h.comp.get_panel_draw_active());
    let pad = recorded_pad();
    let r = drawn(&f);
    assert!(pad.shown);
    assert_eq!(
        (pad.x, pad.y, pad.w, pad.h),
        (r.x, r.y, r.w as u32, r.h as u32),
        "the pad covers the page"
    );
    agent_turn(&f, 16);
    assert!(
        !recorded_mark().shown,
        "no arrow before a press on the page"
    );
    // Press, drag: a blue arrow follows live.
    at(&m, on_page(&f, 20.0, 30.0), false);
    agent_turn(&f, 16);
    at(&m, on_page(&f, 20.0, 30.0), true);
    agent_turn(&f, 16);
    at(&m, on_page(&f, 100.0, 90.0), true);
    agent_turn(&f, 16);
    let mark = recorded_mark();
    assert!(mark.shown);
    assert_eq!(
        (mark.x, mark.y),
        on_page(&f, 100.0, 90.0),
        "the head follows the pointer"
    );
    at(&m, on_page(&f, 160.0, 155.0), true);
    agent_turn(&f, 16);
    assert_eq!(
        (recorded_mark().x, recorded_mark().y),
        on_page(&f, 160.0, 155.0)
    );
    assert!(
        asked_for_node(&slot).is_empty(),
        "nothing asked before the release"
    );
    // The release: the pad goes, the browser is asked what is under the head (no page script), the frame is typed.
    at(&m, on_page(&f, 160.0, 155.0), false);
    agent_turn(&f, 16);
    assert!(!drawing(&f, &id));
    assert!(!recorded_pad().shown);
    let asked = asked_for_node(&slot);
    assert_eq!(asked.len(), 1);
    assert!(
        (asked[0]["x"].as_f64().unwrap() - 160.0).abs() <= 1.0,
        "{asked:?}"
    );
    assert!(
        (asked[0]["y"].as_f64().unwrap() - 155.0).abs() <= 1.0,
        "{asked:?}"
    );
    assert!(
        cdp_calls(&slot)
            .iter()
            .all(|(m, _)| m != "Runtime.evaluate" && m != "Runtime.callFunctionOn"),
        "no page script"
    );
    until_written(&shell, |w| !w.is_empty());
    let typed = String::from_utf8(shell.written()).unwrap();
    assert!(
        typed.starts_with(
            "About this arrow on flare-test.local: > points at: textbox \"Quantity\" > at: "
        ),
        "{typed}"
    );
    assert!(
        typed.ends_with("in a 600 x 400 page > (use browser_look to see it)"),
        "{typed}"
    );
    assert!(
        !typed.contains('\r') && !typed.contains('\u{202e}'),
        "{typed}"
    );
    std::thread::sleep(Duration::from_millis(60)); // sync-ok: test code only
    f.pump();
    assert_eq!(
        String::from_utf8(shell.written()).unwrap(),
        typed,
        "no Enter"
    );
    // The arrow stays on the page for the agent.
    assert!(recorded_mark().shown);
    assert!(f.ctl.borrow().web.user_arrow_of(&id).is_some());
}

#[test]
fn the_next_look_gets_the_persons_arrow_with_a_ref_once_and_then_it_goes() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    claude(&f);
    let id = up(&f, &slot);
    with_shop(&slot);
    let m = mouse(&f);
    press_draw(&f);
    drag(&f, &m, (20.0, 30.0), (160.0, 155.0));
    until_written(&shell, |w| !w.is_empty());
    let v = look(&f, &id);
    let u = &v["user_arrow"];
    assert_eq!(u["ref"], "e2", "the field, as the look lists it");
    assert_eq!(u["role"], "textbox");
    assert_eq!(u["name"], "Quantity");
    assert!((u["to"][0].as_f64().unwrap() - 160.0).abs() <= 1.0, "{u}");
    assert!((u["from"][1].as_f64().unwrap() - 30.0).abs() <= 1.0, "{u}");
    // Seen once: the next look has none, and it is off the page.
    assert!(look(&f, &id).get("user_arrow").is_none());
    assert!(!recorded_mark().shown);
    assert!(f.ctl.borrow().web.user_arrow_of(&id).is_none());
}

#[test]
fn an_arrow_at_an_element_the_look_does_not_list_gets_a_ref_of_its_own() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    claude(&f);
    let id = up(&f, &slot);
    with_shop(&slot);
    // What is under the head is a node no look lists.
    let base = shop(Rc::new(Cell::new(0.0)));
    let mut base = Some(base);
    fake(&slot).state.borrow_mut().cdp = Some(Box::new(move |m: &str, p: &str| {
        match m {
        "DOM.getNodeForLocation" => Ok(json!({"backendNodeId": 77}).to_string()),
        "Accessibility.getPartialAXTree" => Ok(json!({"nodes": [
            {"nodeId": "7", "role": {"value": "img"}, "name": {"value": "Logo"}, "backendDOMNodeId": 77}
        ]})
        .to_string()),
        _ => base.as_mut().map_or(Ok("{}".into()), |b| b(m, p)),
    }
    }));
    let m = mouse(&f);
    press_draw(&f);
    drag(&f, &m, (300.0, 300.0), (320.0, 320.0));
    until_written(&shell, |w| !w.is_empty());
    let typed = String::from_utf8(shell.written()).unwrap();
    assert!(typed.contains("> points at: image \"Logo\""), "{typed}");
    let v = look(&f, &id);
    assert_eq!(
        v["user_arrow"]["ref"], "e3",
        "a ref after the last the look gave"
    );
    assert_eq!(f.ctl.borrow().web.agent_refs(&id), 3);
}

#[test]
fn a_click_with_no_drag_points_at_the_target_from_up_and_left() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    claude(&f);
    let id = up(&f, &slot);
    with_shop(&slot);
    let m = mouse(&f);
    press_draw(&f);
    at(&m, on_page(&f, 160.0, 155.0), false);
    agent_turn(&f, 16);
    // Pressed and let go between two looks.
    m.set(PointerNow {
        left_pressed: true,
        left_down: false,
        ..m.get()
    });
    agent_turn(&f, 16);
    assert!(!drawing(&f, &id));
    until_written(&shell, |w| !w.is_empty());
    let (from, to) = f.ctl.borrow().web.user_arrow_of(&id).unwrap();
    assert!(from.0 < to.0 && from.1 < to.1, "{from:?} {to:?}");
}

#[test]
fn every_cancel_ends_the_draw_mode_with_nothing_asked_and_nothing_typed() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    claude(&f);
    let id = up(&f, &slot);
    with_shop(&slot);
    let m = mouse(&f);
    let check = |what: &str| {
        assert!(!drawing(&f, &id), "{what}");
        assert!(!recorded_pad().shown, "{what}");
        assert!(!recorded_mark().shown, "{what}");
        assert!(!f.h.comp.get_panel_draw_active(), "{what}");
    };
    let mid_drag = || {
        press_draw(&f);
        assert!(drawing(&f, &id));
        at(&m, on_page(&f, 20.0, 30.0), false);
        agent_turn(&f, 16);
        at(&m, on_page(&f, 20.0, 30.0), true);
        agent_turn(&f, 16);
        at(&m, on_page(&f, 90.0, 90.0), true);
        agent_turn(&f, 16);
        assert!(recorded_mark().shown);
    };
    // The button again.
    mid_drag();
    press_draw(&f);
    check("the button again");
    // A right click.
    mid_drag();
    m.set(PointerNow {
        right: true,
        ..m.get()
    });
    agent_turn(&f, 16);
    check("a right click");
    m.set(PointerNow {
        right: false,
        left_down: false,
        left_pressed: false,
        ..m.get()
    });
    // Escape, where it reaches the page: the key filter takes it.
    mid_drag();
    let filter = fake(&slot)
        .state
        .borrow()
        .filter
        .clone()
        .expect("a key filter");
    let esc = KeyPress {
        vk: 0x1B,
        ctrl: false,
        shift: false,
        alt: false,
        down: true,
        repeat: false,
    };
    assert!(filter(&esc), "the page does not get it");
    f.pump();
    check("Escape");
    assert!(!filter(&esc), "with no draw it is the page's again");
    // Escape in Flare's window (a click on the button leaves the keys there).
    mid_drag();
    assert_eq!(
        f.key(KeyEvent::new("Escape", "Escape")),
        KeyOutcome::Consumed
    );
    f.pump();
    check("Escape in the window");
    // A Flare menu over the page.
    mid_drag();
    f.h.comp.set_dialog_open(true);
    f.h.advance(0);
    f.pump();
    check("a menu");
    f.h.comp.set_dialog_open(false);
    f.h.advance(0);
    f.pump();
    // A navigation.
    mid_drag();
    finish(&fake(&slot), &id, TWO);
    f.pump();
    check("a navigation");
    // A pop-up.
    mid_drag();
    fake(&slot).open_popup(&id, "bank.example");
    f.pump();
    f.h.advance(0);
    f.pump();
    check("a pop-up");
    assert!(
        !f.ctl.borrow_mut().web.toggle_draw(&id),
        "no draw over a pop-up"
    );
    f.h.comp.invoke_panel_web_popup_close();
    f.pump();
    f.h.advance(0);
    f.pump();
    // A tab switch.
    mid_drag();
    assert_eq!(f.key(ctrl("t", "KeyT")), KeyOutcome::Consumed);
    f.h.advance(0);
    f.pump();
    check("a tab switch");
    assert!(asked_for_node(&slot).is_empty(), "nothing was asked");
    assert!(shell.written().is_empty(), "nothing was typed");
}

#[test]
fn the_persons_arrow_is_kept_five_minutes_or_until_the_page_moves_on() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    claude(&f);
    let id = up(&f, &slot);
    with_shop(&slot);
    let m = mouse(&f);
    press_draw(&f);
    drag(&f, &m, (20.0, 30.0), (160.0, 155.0));
    until_written(&shell, |w| !w.is_empty());
    assert!(f.ctl.borrow().web.user_arrow_of(&id).is_some());
    f.ctl.borrow_mut().web.skip_clock(USER_ARROW_MS);
    f.ctl.borrow_mut().refresh();
    f.pump();
    assert!(f.ctl.borrow().web.user_arrow_of(&id).is_none());
    assert!(!recorded_mark().shown);
    assert!(
        look(&f, &id).get("user_arrow").is_none(),
        "too old for the look"
    );
    // A navigation takes a fresh one.
    press_draw(&f);
    drag(&f, &m, (20.0, 30.0), (160.0, 155.0));
    f.pump();
    assert!(f.ctl.borrow().web.user_arrow_of(&id).is_some());
    finish(&fake(&slot), &id, TWO);
    f.pump();
    assert!(f.ctl.borrow().web.user_arrow_of(&id).is_none());
    assert!(!recorded_mark().shown);
}

#[test]
fn when_the_browser_names_nothing_under_the_arrow_the_frame_says_so() {
    let (f, _h, shell) = live(Opts::default());
    let slot = with_fake(&f);
    claude(&f);
    up(&f, &slot);
    let mut base = Some(shop(Rc::new(Cell::new(0.0))));
    // The page cannot say what is under the arrow.
    fake(&slot).state.borrow_mut().cdp = Some(Box::new(move |m: &str, p: &str| match m {
        "DOM.getNodeForLocation" => Err("held".into()),
        _ => base.as_mut().map_or(Ok("{}".into()), |b| b(m, p)),
    }));
    let m = mouse(&f);
    press_draw(&f);
    drag(&f, &m, (20.0, 30.0), (160.0, 155.0));
    // The browser named nothing: the frame says so.
    until_written(&shell, |w| !w.is_empty());
    let typed = String::from_utf8(shell.written()).unwrap();
    assert!(
        typed.contains("> points at: nothing Flare could name"),
        "{typed}"
    );
}
