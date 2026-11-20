//! The agent on the device panel (issue #248, PR 5), headless: the finger dot glides to a tap and ripples, a swipe
//! leaves a fading trail, typing wears the keyboard badge, the person's press ends the call `interrupted` and hides
//! the dot at once, the indicators are never in the look's picture, a cancel ends the job and the glow, and still mode
//! answers input calls `unavailable` with its reason (rules 1320, 1321). The controller runs over the fake source and
//! the software renderer, on a clock the test turns.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use base64::Engine as _;
use flare_ctl::{CancelFlag, CtlError};
use flare_device::fake::{FAKE_PNG, FakeSource, Recorded};
use flare_device::job::{At, DeviceOp, Direction, SwipeTo};
use flare_device::source::Phase;
use flare_surfaces::PanelKind;
use flare_surfaces::agent_cursor::{APPEAR_MS, GLIDE_MAX_MS};
use serde_json::Value;
use slint::Model as _;

use crate::AppWindow;
use crate::appctl::AppCtl;
use crate::device_ctl::{AgentCall, AgentReq, DeviceCtl, Effect};
use crate::panel_ctl::PanelCtl;
use crate::ui_harness::{Frame, Harness};

const TARGET: &str = "sim/00000000-0000-4000-8000-000000000001";

type Answer = Receiver<Result<Value, CtlError>>;

/// The controller over the fake on a turnable clock, one tab with its device panel live, motion on.
struct Rig {
    h: Harness<AppWindow>,
    panels: PanelCtl,
    dev: DeviceCtl,
    clock: Rc<Cell<u64>>,
}

impl Rig {
    fn new() -> Rig {
        let post: crate::appctl::Post = Arc::new(|_| {});
        let mut panels = PanelCtl::new(Arc::clone(&post));
        panels.set_device_offered(true);
        let clock = Rc::new(Cell::new(1_000));
        let c = Rc::clone(&clock);
        let mut dev = DeviceCtl::new(post, Rc::new(move || c.get()), Some(FakeSource::default()));
        dev.set_motion(Rc::new(|| true));
        let h = Harness::new(1.0, 1200, 800, || {
            AppWindow::new().expect("the window builds")
        });
        AppCtl::push_panel_limits(&h.comp);
        h.advance(0);
        let mut r = Rig {
            h,
            panels,
            dev,
            clock,
        };
        r.panels.show("t1", PanelKind::Device, TARGET);
        r.front();
        r.dev.setup_clicked(); // Boot
        r.front();
        assert!(r.h.comp.get_panel_device_has_frame(), "live first");
        r
    }

    fn front(&mut self) {
        let names: HashMap<String, String> = [("t1".into(), "Flare app".into())].into();
        self.panels.sync(&self.h.comp, Some("t1"));
        for e in self.dev.update(&self.panels, Some("t1"), false, names) {
            if let Effect::SetTarget { tab, target } = e {
                self.panels.set_device_target(&tab, &target);
            }
        }
        self.dev.push(&self.h.comp);
        self.h.advance(0);
    }

    fn call(&mut self, req: AgentReq) -> (Answer, CancelFlag) {
        let (tx, rx) = channel();
        let cancel = CancelFlag::default();
        self.dev.start_agent(AgentCall {
            tab: "t1".into(),
            req,
            reply: tx,
            cancel: cancel.clone(),
            label: "Claude".into(),
        });
        self.show();
        (rx, cancel)
    }

    /// The clock moves on by `ms` and the agent's tick runs (as its timer would).
    fn at(&mut self, ms: u64) {
        self.clock.set(self.clock.get() + ms);
        self.dev.agent_tick();
        self.show();
    }

    fn show(&mut self) {
        self.dev.push(&self.h.comp);
        self.h.advance(0);
    }

    fn dot(&self) -> Option<(f32, f32)> {
        let c = &self.h.comp;
        c.get_panel_device_dot()
            .then(|| (c.get_panel_device_dot_x(), c.get_panel_device_dot_y()))
    }

    fn touches(&self) -> Vec<(Phase, f64, f64)> {
        self.dev
            .fake()
            .unwrap()
            .inputs()
            .iter()
            .filter_map(|r| match r {
                Recorded::Touch(_, t) => Some((t.phase, t.x, t.y)),
                _ => None,
            })
            .collect()
    }
}

fn tap(x: f64, y: f64) -> AgentReq {
    AgentReq::Act(DeviceOp::Tap {
        at: At::Point(x, y),
        hold_ms: None,
    })
}

/// Waits (on the test's clock) for the call's answer, at most `ms`.
fn answer(r: &mut Rig, rx: &Answer, ms: u64) -> Result<Value, CtlError> {
    let mut spent = 0;
    loop {
        if let Ok(a) = rx.try_recv() {
            return a;
        }
        assert!(spent < ms, "no answer within {ms} ms");
        r.at(16);
        spent += 16;
    }
}

/// The agent's orange (the dot's ring at 80%) over the fake's frame colours, in the dot's square.
fn orange_near(f: &Frame, x: f32, y: f32) -> usize {
    let (x, y) = (x.round() as u32, y.round() as u32);
    f.count_where(
        x.saturating_sub(30),
        y.saturating_sub(30),
        (x + 30).min(f.w),
        (y + 30).min(f.h),
        |p| p[0] > 180 && (90..170).contains(&p[1]) && p[2] < 120,
    )
}

#[test]
fn the_finger_dot_glides_to_the_tap_and_ripples() {
    let mut r = Rig::new();
    // The first tap: the dot appears at its point (no glide from nowhere), holds the touch for its appearance.
    let (rx, _) = r.call(tap(100.0, 200.0));
    assert_eq!(
        r.dot(),
        Some((100.0, 200.0)),
        "the dot appears at the point"
    );
    assert!(r.touches().is_empty(), "the touch waits for the dot");
    assert!(
        r.h.comp.get_panel_device_glow() > 0.0,
        "the glow is up while the agent works"
    );
    assert_eq!(r.h.comp.get_panel_device_tag().as_str(), "Claude");
    r.at(APPEAR_MS + 16);
    let t = r.touches();
    assert_eq!(t.first().map(|t| t.0), Some(Phase::Start), "{t:?}");
    assert!(
        r.h.comp.get_panel_device_ripples().iter().count() >= 1,
        "the touch ripples"
    );
    // The ring is drawn over the picture at the dot.
    let f = r.h.render();
    let (wx, wy) = (
        r.h.comp.get_panel_device_dot_wx(),
        r.h.comp.get_panel_device_dot_wy(),
    );
    assert!(orange_near(&f, wx, wy) > 20, "the dot is drawn at the tap");
    let a = answer(&mut r, &rx, 2_000).expect("the tap answers");
    assert_eq!(a["op"], "tap");
    // The second tap glides from where the dot rests, with the agent cursor's timing.
    let (rx, _) = r.call(tap(300.0, 600.0));
    assert_eq!(r.dot(), Some((100.0, 200.0)), "it starts where it rested");
    let n = r.touches().len();
    r.at(200);
    let (x, y) = r.dot().expect("drawn mid-glide");
    assert!(x > 100.0 && x < 300.0 && y > 200.0 && y < 600.0, "{x} {y}");
    assert_eq!(r.touches().len(), n, "no touch mid-glide");
    r.at(GLIDE_MAX_MS);
    assert_eq!(r.dot(), Some((300.0, 600.0)), "it lands on the tap");
    assert!(r.touches().len() > n, "the touch went after the glide");
    answer(&mut r, &rx, 2_000).expect("the second tap answers");
    // The hold, then the fade: gone a little after the last action.
    r.at(1_500 + 300);
    assert_eq!(r.dot(), None, "the dot fades out after the hold");
    assert_eq!(r.h.comp.get_panel_device_glow(), 0.0);
}

#[test]
fn a_swipe_leaves_a_fading_trail() {
    let mut r = Rig::new();
    let (rx, _) = r.call(AgentReq::Act(DeviceOp::Swipe {
        from: Some(At::Point(200.0, 700.0)),
        to: SwipeTo::Direction(Direction::Up),
        duration_ms: Some(300),
    }));
    r.at(APPEAR_MS + 16);
    let mut longest = 0;
    for _ in 0..12 {
        r.at(16);
        longest = longest.max(r.h.comp.get_panel_device_trail().iter().count());
    }
    assert!(longest >= 3, "the dot leaves a trail of points ({longest})");
    let trail: Vec<_> = r.h.comp.get_panel_device_trail().iter().collect();
    // Oldest first, fading with age.
    if trail.len() >= 2 {
        assert!(trail[0].a <= trail[trail.len() - 1].a, "{trail:?}");
    }
    let (_, y) = r.dot().expect("the dot follows the path");
    assert!(y < 700.0, "the dot moved up with the finger ({y})");
    answer(&mut r, &rx, 2_000).expect("the swipe answers");
    r.at(400);
    assert_eq!(
        r.h.comp.get_panel_device_trail().iter().count(),
        0,
        "the trail fades out"
    );
    let t = r.touches();
    assert_eq!(t.first().map(|t| t.0), Some(Phase::Start));
    assert_eq!(t.last().map(|t| t.0), Some(Phase::End));
}

#[test]
fn typing_shows_the_keyboard_badge() {
    let mut r = Rig::new();
    let (rx, _) = r.call(AgentReq::Act(DeviceOp::Type {
        text: "hello".into(),
        at: None,
        submit: true,
    }));
    assert_eq!(
        r.h.comp.get_panel_device_badge().as_str(),
        "Claude Typing...",
        "the badge names the agent"
    );
    assert!(r.h.comp.get_panel_device_badge_shown());
    assert!(
        !r.h.comp.get_panel_device_dot(),
        "no dot: the badge sits at the bottom centre"
    );
    // It is drawn: orange at the bottom centre of the picture.
    let f = r.h.render();
    let c = &r.h.comp;
    let (px, py, pw, ph) = (
        c.get_panel_device_pic_x(),
        c.get_panel_device_pic_y(),
        c.get_panel_device_pic_w(),
        c.get_panel_device_pic_h(),
    );
    let hits = f.count_where(
        (px + pw / 2.0 - 40.0) as u32,
        (py + ph - 60.0) as u32,
        (px + pw / 2.0 + 40.0) as u32,
        (py + ph - 20.0) as u32,
        |p| Frame::near(p, [0xfe, 0x8f, 0x34, 255], 12),
    );
    assert!(hits > 50, "the badge is drawn ({hits})");
    answer(&mut r, &rx, 3_000).expect("typing answers");
    r.at(flare_device::indicator::BADGE_MIN_MS);
    assert_eq!(
        r.h.comp.get_panel_device_badge().as_str(),
        "",
        "the badge goes with the call"
    );
    // The keys went as HID usages, then Return.
    let keys: Vec<(u32, bool)> = r
        .dev
        .fake()
        .unwrap()
        .inputs()
        .iter()
        .filter_map(|i| match i {
            Recorded::Key(_, k) => Some((k.usage, k.down)),
            _ => None,
        })
        .collect();
    assert_eq!(keys.len(), 12, "{keys:?}");
    assert_eq!(keys[10], (flare_device::keymap::usage::RETURN, true));
    // A key press wears the key's name.
    let (rx, _) = r.call(AgentReq::Act(DeviceOp::Press(
        flare_device::job::Press::Key("return".into()),
    )));
    assert_eq!(r.h.comp.get_panel_device_badge().as_str(), "Claude Return");
    answer(&mut r, &rx, 2_000).expect("the key answers");
}

#[test]
fn the_persons_press_ends_the_agent_call_interrupted_and_hides_the_dot() {
    let mut r = Rig::new();
    let (rx, _) = r.call(AgentReq::Act(DeviceOp::Tap {
        at: At::Point(150.0, 300.0),
        hold_ms: Some(5_000),
    }));
    r.at(APPEAR_MS + 16);
    assert!(r.dot().is_some());
    assert!(
        r.h.comp.get_panel_device_dot_held(),
        "a long press holds a filled dot"
    );
    // The person presses in the panel: the call ends `interrupted`, the finger is let go of, the dot goes at once.
    let (bw, bh) = (r.h.comp.get_panel_device_w(), r.h.comp.get_panel_device_h());
    r.dev.page_pointer(&r.h.comp, 0, bw / 2.0, bh / 2.0, 0, 0);
    r.show();
    let e = rx.try_recv().expect("answered at once").unwrap_err();
    assert_eq!(e.code, "interrupted");
    assert!(e.message.contains("took over"), "{}", e.message);
    assert_eq!(r.dot(), None, "the dot is hidden at once");
    assert_eq!(r.h.comp.get_panel_device_glow(), 0.0, "and the glow");
    // The agent's finger was let go of before the person's press went down.
    let t = r.touches();
    let ends = t.iter().filter(|t| t.0 == Phase::End).count();
    assert_eq!(ends, 1, "the held finger was let go of: {t:?}");
    assert_eq!(r.dev.probe()["takeovers"], 1);
    let st = r.dev.agent_state("t1");
    assert_eq!(st["took_over_last"], true);
}

#[test]
fn indicators_are_never_in_the_look_picture() {
    let mut r = Rig::new();
    let (rx, _) = r.call(AgentReq::Act(DeviceOp::Tap {
        at: At::Point(150.0, 300.0),
        hold_ms: Some(3_000),
    }));
    r.at(APPEAR_MS + 16);
    assert!(r.dot().is_some(), "the dot is drawn over the picture");
    // A look while the dot shows (from another caller's view: the controller itself) gets the source's own picture.
    let (tx, look) = channel();
    r.dev.start_agent(AgentCall {
        tab: "t1".into(),
        req: AgentReq::Look { tree: true },
        reply: tx,
        cancel: CancelFlag::default(),
        label: "Claude".into(),
    });
    let v = look.try_recv().expect("the look answers").expect("a look");
    let data = v["image"]["data"].as_str().unwrap();
    let png = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    assert_eq!(
        png, FAKE_PNG,
        "the sim host's picture as it is: no dot, no ring, no badge"
    );
    assert_eq!(v["image"]["mime"], "image/png");
    assert_eq!(v["screen"]["w"], 402.0);
    let refs: Vec<&str> = v["tree"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["label"].as_str())
        .collect();
    assert!(refs.contains(&"Tap me"), "{refs:?}");
    drop(rx);
}

#[test]
fn a_cancel_ends_the_job_and_hides_the_glow() {
    let mut r = Rig::new();
    let (rx, cancel) = r.call(AgentReq::Act(DeviceOp::Tap {
        at: At::Point(150.0, 300.0),
        hold_ms: Some(5_000),
    }));
    r.at(APPEAR_MS + 16);
    assert!(r.h.comp.get_panel_device_glow() > 0.0);
    // The caller went away (rule 229): the job ends at the next tick, its finger let go of.
    cancel.set();
    r.at(16);
    let e = rx.try_recv().expect("answered").unwrap_err();
    assert_eq!(e.code, "interrupted");
    assert!(r.touches().iter().any(|t| t.0 == Phase::End));
    // The glow holds a moment, then fades: never left on.
    r.at(1_500 + 300);
    assert_eq!(r.h.comp.get_panel_device_glow(), 0.0);
    assert!(!r.dev.agent_tick(), "nothing left to tick");
}

#[test]
fn still_mode_answers_input_tools_unavailable_with_the_reason() {
    let mut r = Rig::new();
    r.dev
        .fake_mut()
        .unwrap()
        .go_broken("the simulator helper stopped three times");
    r.front();
    let (rx, _) = r.call(tap(100.0, 100.0));
    let e = rx.try_recv().expect("answered at once").unwrap_err();
    assert_eq!(e.code, "unavailable");
    assert!(
        e.message
            .contains("the simulator helper stopped three times"),
        "{}",
        e.message
    );
    assert!(r.touches().is_empty(), "nothing was sent");
    assert_eq!(r.dot(), None, "no dot for a refused call");
    // device_state says why too.
    assert_eq!(
        r.dev.agent_state("t1")["still"],
        "the simulator helper stopped three times"
    );
}
