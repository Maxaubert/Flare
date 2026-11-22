//! The person's input in the device panel (issue #248, PR 4a), through the real controller and the window's own
//! pointer events: a click is a touch start and end in device points, a wheel a short drag at the pointer, points in
//! the letterbox's bars are dropped, holding the splitter is no takeover (rule 1320), and the stream pauses while the
//! tab is behind and resumes with the current frame. The source is the fake the app has under `--e2e`.

use flare_device::fake::Recorded;
use flare_device::source::{Phase, Touch};

use super::*;

const TARGET: &str = "sim/00000000-0000-4000-8000-000000000001";

/// Two tabs, the first in front with its device panel live (booted with the panel's own Boot).
fn live_device() -> (Fx, String, String) {
    let f = fx_with(Opts {
        saved: vec![("C:\\a", None), ("C:\\b", None)],
        ..Opts::default()
    });
    f.start_restore_and_wait();
    let ids: Vec<String> = f
        .ctl
        .borrow()
        .model()
        .state
        .tabs
        .iter()
        .map(|t| t.id.clone())
        .collect();
    let (a, b) = (ids[0].clone(), ids[1].clone());
    f.h.comp.invoke_overflow_picked(a.clone().into());
    assert!(f.ctl.borrow_mut().device_open(&a, Some(TARGET)));
    f.h.advance(0);
    assert_eq!(f.h.comp.get_panel_kind(), 5);
    assert_eq!(f.h.comp.get_panel_device_button().as_str(), "Boot");
    f.h.comp.invoke_panel_device_setup();
    f.h.advance(0);
    assert!(
        f.h.comp.get_panel_device_has_frame(),
        "live with its first frame"
    );
    (f, a, b)
}

fn touches(f: &Fx) -> Vec<Touch> {
    f.ctl
        .borrow_mut()
        .device_mut()
        .fake()
        .unwrap()
        .inputs()
        .iter()
        .filter_map(|r| match r {
            Recorded::Touch(_, t) => Some(*t),
            _ => None,
        })
        .collect()
}

fn pic(f: &Fx) -> (f32, f32, f32, f32) {
    let c = &f.h.comp;
    (
        c.get_panel_device_pic_x(),
        c.get_panel_device_pic_y(),
        c.get_panel_device_pic_w(),
        c.get_panel_device_pic_h(),
    )
}

#[test]
fn a_click_becomes_touch_start_and_end_in_device_points() {
    let (f, ..) = live_device();
    let (x, y, w, h) = pic(&f);
    // A quarter across and a third down the picture is a quarter across and a third down the device.
    f.h.click(x + w / 4.0, y + h / 3.0);
    let t = touches(&f);
    assert_eq!(t.len(), 2, "{t:?}");
    assert_eq!((t[0].phase, t[1].phase), (Phase::Start, Phase::End));
    for touch in &t {
        assert!((touch.x - 402.0 / 4.0).abs() < 1.0, "{touch:?}");
        assert!((touch.y - 874.0 / 3.0).abs() < 1.0, "{touch:?}");
        assert_eq!(touch.second, None);
    }
    // The press gave the panel the keys: a letter goes to the device as its HID key.
    assert!(f.h.comp.get_panel_device_focused());
    f.h.key_text("a");
    let keys: Vec<_> = f
        .ctl
        .borrow_mut()
        .device_mut()
        .fake()
        .unwrap()
        .inputs()
        .iter()
        .filter_map(|r| match r {
            Recorded::Key(_, k) => Some((k.usage, k.down)),
            _ => None,
        })
        .collect();
    assert_eq!(keys, [(0x04, true), (0x04, false)]);
    // A right click is nothing.
    f.h.right_click(x + w / 2.0, y + h / 2.0);
    assert_eq!(touches(&f).len(), 2);
}

#[test]
fn a_wheel_becomes_a_short_drag_at_the_pointer() {
    let (f, ..) = live_device();
    let (x, y, w, h) = pic(&f);
    let scale = f64::from(w) / 402.0;
    f.h.wheel(x + w / 2.0, y + h / 2.0, 0.0, -60.0);
    let t = touches(&f);
    assert_eq!(
        t.len(),
        2 + crate::device_ctl::WHEEL_STEPS as usize,
        "{t:?}"
    );
    assert_eq!(t[0].phase, Phase::Start);
    assert!(t[1..t.len() - 1].iter().all(|m| m.phase == Phase::Move));
    let end = t.last().unwrap();
    assert_eq!(end.phase, Phase::End);
    assert!(
        (t[0].x - 201.0).abs() < 1.0 && (t[0].y - 437.0).abs() < 1.0,
        "{:?}",
        t[0]
    );
    // The fingers move with the wheel, by its distance in device points.
    assert!((end.x - t[0].x).abs() < 0.01);
    assert!((end.y - (t[0].y - 60.0 / scale)).abs() < 0.5, "{end:?}");
}

#[test]
fn points_outside_the_screen_are_dropped() {
    let (f, ..) = live_device();
    let (x, y, w, h) = pic(&f);
    let (bx, ..) = (f.h.comp.get_panel_device_x(), ());
    assert!(
        x - bx > 8.0,
        "the picture is letterboxed with bars at the sides"
    );
    // In the bar left of the picture: nothing.
    f.h.click((bx + x) / 2.0, y + h / 2.0);
    assert!(touches(&f).is_empty());
    // A drag that starts inside and leaves the screen: its moves outside are dropped, it ends where it left.
    let from = (x + w / 2.0, y + h / 2.0);
    f.h.press(from.0, from.1, slint::platform::PointerEventButton::Left);
    f.clock.set(100);
    f.h.move_to(from.0 - 10.0, from.1);
    f.clock.set(200);
    f.h.move_to(bx + 1.0, from.1);
    f.h.release(bx + 1.0, from.1, slint::platform::PointerEventButton::Left);
    let t = touches(&f);
    assert_eq!(t.first().map(|t| t.phase), Some(Phase::Start));
    assert_eq!(t.last().map(|t| t.phase), Some(Phase::End));
    assert!(
        t.iter()
            .all(|t| (0.0..=402.0).contains(&t.x) && (0.0..=874.0).contains(&t.y)),
        "{t:?}"
    );
    let last_move = t.iter().rev().find(|t| t.phase == Phase::Move).unwrap();
    assert_eq!(
        (t.last().unwrap().x, t.last().unwrap().y),
        (last_move.x, last_move.y)
    );
}

#[test]
fn holding_the_splitter_is_not_a_takeover() {
    let (f, a, _) = live_device();
    let (x, y, w, h) = pic(&f);
    let (bx, by) = (f.h.comp.get_panel_device_x(), f.h.comp.get_panel_device_y());
    // A long press of the agent's runs in the tab (PR 5): it holds its finger down for 5 s.
    let (tx, rx) = std::sync::mpsc::channel();
    f.ctl
        .borrow_mut()
        .device_mut()
        .start_agent(crate::device_ctl::AgentCall {
            tab: a.clone(),
            req: crate::device_ctl::AgentReq::Act(flare_device::job::DeviceOp::Tap {
                at: flare_device::job::At::Point(100.0, 100.0),
                hold_ms: Some(5_000),
            }),
            reply: tx,
            cancel: flare_ctl::CancelFlag::default(),
            label: "Claude".into(),
        });
    // The splitter is held: a press in the panel ends nothing.
    f.h.comp.set_panel_splitting(true);
    f.ctl.borrow_mut().refresh();
    f.h.comp
        .invoke_panel_device_pointer(0, x - bx + w / 2.0, y - by + h / 2.0, 0, 0);
    f.h.comp
        .invoke_panel_device_pointer(1, x - bx + w / 2.0, y - by + h / 2.0, 0, 0);
    assert!(rx.try_recv().is_err(), "the call still runs");
    // Let go: the person's press takes over, and the call ends `interrupted`.
    f.h.comp.set_panel_splitting(false);
    f.ctl.borrow_mut().refresh();
    f.h.click(x + w / 2.0, y + h / 2.0);
    let ended = rx.try_recv().expect("the call was answered");
    assert_eq!(ended.unwrap_err().code, "interrupted");
    assert_eq!(f.ctl.borrow().device_probe()["takeovers"], 1);
}

#[test]
fn the_stream_pauses_when_the_tab_goes_behind_and_resumes_with_the_current_frame() {
    let (f, a, b) = live_device();
    let pauses = |f: &Fx| -> Vec<bool> {
        f.ctl
            .borrow_mut()
            .device_mut()
            .fake()
            .unwrap()
            .inputs()
            .iter()
            .filter_map(|r| match r {
                Recorded::Pause(_, p) => Some(*p),
                _ => None,
            })
            .collect()
    };
    let shown = f.ctl.borrow().device_probe()["frames_shown"]
        .as_u64()
        .unwrap();
    // The other tab in front: the device's stream pauses (input and the rest go on).
    f.h.comp.invoke_overflow_picked(b.clone().into());
    f.h.advance(0);
    assert_eq!(pauses(&f), [true]);
    assert_ne!(
        f.h.comp.get_panel_kind(),
        5,
        "the other tab shows no device"
    );
    assert!(
        f.ctl.borrow().device_probe()["paused"].is_null()
            || f.ctl.borrow().device_probe()["tab"].is_null()
    );
    // Back: it resumes, and the current frame shows at once.
    f.h.comp.invoke_overflow_picked(a.clone().into());
    f.h.advance(0);
    assert_eq!(pauses(&f), [true, false]);
    let p = f.ctl.borrow().device_probe();
    assert_eq!(p["paused"], false);
    assert!(
        p["frames_shown"].as_u64().unwrap() > shown,
        "a new frame on resume"
    );
    assert!(f.h.comp.get_panel_device_has_frame());
    // Minimised: paused; restored: live again.
    f.ctl.borrow_mut().on_minimised(true);
    assert_eq!(pauses(&f), [true, false, true]);
    f.ctl.borrow_mut().on_minimised(false);
    assert_eq!(pauses(&f), [true, false, true, false]);
    // The panel closed: paused too.
    f.ctl
        .borrow_mut()
        .on_key(&KeyEvent::new("P", "KeyP").ctrl().shift());
    f.ctl.borrow_mut().refresh();
    assert_eq!(pauses(&f).last(), Some(&true));
}

/// Android PR A7: the same window events on the Android fake (`emu/Flare_Pixel`, 411 x 914 dp): a click is a touch
/// start and end in dp, a letter its HID key, and the panel's own Back reaches the device.
#[test]
fn an_android_device_takes_clicks_in_dp_and_keys() {
    let f = fx_with(Opts {
        saved: vec![("C:\\a", None)],
        ..Opts::default()
    });
    f.start_restore_and_wait();
    let a = f.ctl.borrow().model().state.tabs[0].id.clone();
    f.ctl
        .borrow_mut()
        .device_mut()
        .use_fake(flare_device::fake::FakeSource::android());
    f.h.comp.invoke_overflow_picked(a.clone().into());
    assert!(f.ctl.borrow_mut().device_open(&a, Some("emu/Flare_Pixel")));
    f.h.advance(0);
    assert_eq!(f.h.comp.get_panel_device_button().as_str(), "Boot");
    f.h.comp.invoke_panel_device_setup();
    f.h.advance(0);
    assert!(f.h.comp.get_panel_device_has_frame(), "live");
    assert!(f.h.comp.get_panel_device_android());
    let (x, y, w, h) = pic(&f);
    f.h.click(x + w / 4.0, y + h / 3.0);
    let t = touches(&f);
    assert_eq!(t.len(), 2, "{t:?}");
    assert_eq!((t[0].phase, t[1].phase), (Phase::Start, Phase::End));
    for touch in &t {
        assert!((touch.x - 411.0 / 4.0).abs() < 1.0, "{touch:?}");
        assert!((touch.y - 914.0 / 3.0).abs() < 1.0, "{touch:?}");
    }
    f.h.key_text("a");
    let back = f.ctl.borrow_mut().device_mut().act("back");
    assert!(back.is_empty(), "{back:?}");
    let ins: Vec<String> = f
        .ctl
        .borrow_mut()
        .device_mut()
        .fake()
        .unwrap()
        .inputs()
        .iter()
        .filter_map(|r| match r {
            Recorded::Key(t, k) if t.name() == "emu/Flare_Pixel" => {
                Some(format!("key {} {}", k.usage, k.down))
            }
            Recorded::Button(t, b) if t.name() == "emu/Flare_Pixel" => {
                Some(format!("button {}", b.name()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(ins, ["key 4 true", "key 4 false", "button back"], "{ins:?}");
}
