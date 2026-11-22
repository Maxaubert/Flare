//! The device panel (issue #248, kind 5, PR 4a), headless: it lays out like a web panel with a header at every
//! scale in both looks, the picture is letterboxed to the device's aspect, each setup state says one line with at
//! most one button (rule 1318), no Xcode offers Get Xcode, still pictures wear their badge and reason, the CPU path
//! its slow mark, and a second tab on a device another tab drives is view only. The controller runs over the fake
//! source and the software renderer (which always takes the CPU path).

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use flare_device::fake::{FakeSource, Recorded};
use flare_device::geom::{Letterbox, Size};
use flare_surfaces::PanelKind;
use slint::{Global, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::appctl::AppCtl;
use crate::device_ctl::{DeviceCtl, Effect, Setup, SetupButton, XCODE_STORE, Xcode};
use crate::panel_ctl::PanelCtl;
use crate::ui_harness::{Frame, Harness};
use crate::{AppWindow, DeviceRow, Theme};

const W: u32 = 1200;
const H: u32 = 800;
/// 100, 150, 225 and 300 percent.
const SCALES: [f32; 4] = [1.0, 1.5, 2.25, 3.0];
const TARGET: &str = "sim/00000000-0000-4000-8000-000000000001";
/// The Theme's `widget-warn` (the still badge's dot and the slow mark).
const WARN: [u8; 4] = [0xf0, 0xb4, 0x5a, 255];

fn window(scale: f32) -> Harness<AppWindow> {
    let h = Harness::new(scale, W, H, || AppWindow::new().expect("the window builds"));
    AppCtl::push_panel_limits(&h.comp);
    h.advance(0);
    h
}

fn solid(w: u32, h: u32, px: [u8; 4]) -> Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    for p in buf.make_mut_slice() {
        *p = Rgba8Pixel {
            r: px[0],
            g: px[1],
            b: px[2],
            a: px[3],
        };
    }
    Image::from_rgba8(buf)
}

/// A live device on show, as the controller pushes it.
fn open_live(h: &Harness<AppWindow>, pt: (f32, f32)) {
    h.comp.set_panel_open(true);
    h.comp.set_panel_kind(5);
    h.comp.set_panel_title("iPhone 18 Pro".into());
    h.comp.set_panel_device_state(0);
    h.comp.set_panel_device_pt_w(pt.0);
    h.comp.set_panel_device_pt_h(pt.1);
    h.comp.set_panel_device_radius_pt(55.0);
    h.comp
        .set_panel_device_frame(solid(pt.0 as u32, pt.1 as u32, [40, 90, 200, 255]));
    h.comp.set_panel_device_has_frame(true);
    h.comp.set_panel_device_controls(true);
    h.advance(0);
}

/// A setup state on show, as the controller pushes it.
fn show_setup(h: &Harness<AppWindow>, s: &Setup) {
    let v = s.view(false);
    h.comp.set_panel_open(true);
    h.comp.set_panel_kind(5);
    h.comp.set_panel_device_has_frame(false);
    h.comp.set_panel_device_controls(false);
    h.comp.set_panel_device_state(v.state);
    h.comp.set_panel_device_line(v.line.into());
    h.comp
        .set_panel_device_button(v.button.map_or("", SetupButton::label).into());
    let rows: Vec<DeviceRow> = if v.state == 2 {
        vec![DeviceRow {
            name: "iPhone 18 Pro".into(),
            os: "iOS 27.2".into(),
            booted: true,
        }]
    } else {
        Vec::new()
    };
    h.comp
        .set_panel_device_rows(ModelRc::from(Rc::new(VecModel::from(rows))));
    h.advance(0);
}

fn box_of(h: &Harness<AppWindow>) -> (f32, f32, f32, f32) {
    let c = &h.comp;
    (
        c.get_panel_device_x(),
        c.get_panel_device_y(),
        c.get_panel_device_w(),
        c.get_panel_device_h(),
    )
}

fn count_in(f: &Frame, s: f32, r: (f32, f32, f32, f32), want: [u8; 4], tol: u8) -> usize {
    let p = |v: f32| (v * s).round().max(0.0) as u32;
    f.count_where(
        p(r.0),
        p(r.1),
        p(r.0 + r.2).min(f.w),
        p(r.1 + r.3).min(f.h),
        |px| Frame::near(px, want, tol),
    )
}

#[test]
fn kind_five_lays_out_like_the_web_panel_at_every_scale() {
    for s in SCALES {
        for light in [false, true] {
            let h = window(s);
            Theme::get(&h.comp).set_light(light);
            let at = format!("scale {s}, light {light}");
            // A mini-app: a web panel with a header (a page's bar is its head, the device has a header).
            h.comp.set_panel_open(true);
            h.comp.set_panel_kind(3);
            h.comp.set_panel_web_app(true);
            h.advance(0);
            let web = (
                h.comp.get_panel_web_x(),
                h.comp.get_panel_web_y(),
                h.comp.get_panel_web_w(),
                h.comp.get_panel_web_h(),
            );
            let web_head = h.comp.get_panel_header_h();
            h.comp.set_panel_web_app(false);
            open_live(&h, (402.0, 874.0));
            let dev = box_of(&h);
            assert!(h.comp.get_panel_shown(), "{at}");
            assert_eq!(
                h.comp.get_panel_header_h(),
                web_head,
                "{at}: the same header"
            );
            assert_eq!(h.comp.get_panel_bar_h(), 0.0, "{at}: no address bar");
            for (a, b, what) in [
                (dev.0, web.0, "x"),
                (dev.1, web.1, "y"),
                (dev.2, web.2, "w"),
                (dev.3, web.3, "h"),
            ] {
                assert!(
                    (a - b).abs() < 0.01,
                    "{at}: the box's {what} is the page's ({a} vs {b})"
                );
            }
            // The box's edges sit on whole device pixels, as the rest of the chrome.
            for v in [dev.0, dev.1, dev.0 + dev.2, dev.1 + dev.3] {
                assert!(
                    ((v * s) - (v * s).round()).abs() < 0.02,
                    "{at}: {v} on whole pixels"
                );
            }
            // The header's device buttons are inside the slot, left of the close.
            assert!(h.comp.get_panel_device_controls_shown(), "{at}");
            let slot_right =
                h.comp.get_page_left() + h.comp.get_panel_slot_x() + h.comp.get_panel_visible_w();
            let rot = h.comp.get_panel_device_rotate_x();
            let home = h.comp.get_panel_device_home_x();
            assert!(
                home < rot && rot + 24.0 < slot_right - 24.0,
                "{at}: {home} {rot} {slot_right}"
            );
            // The picture is drawn: its colour fills its middle.
            let f = h.render();
            let pic = (
                h.comp.get_panel_device_pic_x() + 30.0,
                h.comp.get_panel_device_pic_y() + 60.0,
                h.comp.get_panel_device_pic_w() - 60.0,
                h.comp.get_panel_device_pic_h() - 120.0,
            );
            assert!(
                count_in(&f, s, pic, [40, 90, 200, 255], 6) > 100,
                "{at}: the picture shows"
            );
        }
    }
}

#[test]
fn the_picture_is_letterboxed_to_the_device_aspect() {
    for s in SCALES {
        let h = window(s);
        for pt in [(402.0f32, 874.0f32), (874.0, 402.0)] {
            open_live(&h, pt);
            let (bx, by, bw, bh) = box_of(&h);
            let lb = Letterbox::fit(
                Size::new(f64::from(bw), f64::from(bh)),
                Size::new(f64::from(pt.0), f64::from(pt.1)),
            )
            .unwrap();
            let c = &h.comp;
            let at = format!("scale {s}, {pt:?}");
            let near = |a: f32, b: f64| (f64::from(a) - b).abs() < 0.05;
            assert!(near(c.get_panel_device_pic_w(), lb.shown().w), "{at}");
            assert!(near(c.get_panel_device_pic_h(), lb.shown().h), "{at}");
            assert!(near(c.get_panel_device_pic_x() - bx, lb.x), "{at}");
            assert!(near(c.get_panel_device_pic_y() - by, lb.y), "{at}");
            let aspect = c.get_panel_device_pic_w() / c.get_panel_device_pic_h();
            assert!(
                (aspect - pt.0 / pt.1).abs() < 0.005,
                "{at}: aspect {aspect}"
            );
            // It fills one side of the box and is centred on the other.
            assert!(
                (c.get_panel_device_pic_w() - bw).abs() < 0.05
                    || (c.get_panel_device_pic_h() - bh).abs() < 0.05,
                "{at}"
            );
        }
    }
}

#[test]
fn each_setup_state_shows_one_line_and_at_most_one_button() {
    let name = || "iPhone 18 Pro".to_string();
    let all = [
        Setup::NoXcode,
        Setup::NotSetUp,
        Setup::NoRuntime,
        Setup::Reading,
        Setup::Choose,
        Setup::Booting {
            name: name(),
            secs: 4,
        },
        Setup::Live { name: name() },
        Setup::ShutDown {
            name: name(),
            again: true,
        },
        Setup::ShutDown {
            name: name(),
            again: false,
        },
        Setup::Failed {
            why: "the device is still booting".into(),
        },
        // Android's (Android plan PR A4).
        Setup::SdkMissing,
        Setup::Licence {
            text: "Terms and Conditions".into(),
            download_mb: 2538,
        },
        Setup::Downloading {
            done_mb: 120,
            total_mb: 2538,
        },
        Setup::NoHypervisor,
        Setup::RestartNeeded,
        Setup::NoAccess,
        Setup::Preparing {
            name: "Flare Pixel".into(),
        },
    ];
    let h = window(1.5);
    let setups = Rc::new(Cell::new(0));
    let picks = Rc::new(Cell::new(-1));
    {
        let setups = setups.clone();
        h.comp
            .on_panel_device_setup(move || setups.set(setups.get() + 1));
        let picks = picks.clone();
        h.comp.on_panel_device_pick(move |i| picks.set(i));
    }
    for s in all {
        let v = s.view(false);
        show_setup(&h, &s);
        let at = format!("{s:?}");
        assert!(
            !v.line.is_empty() && !v.line.contains('\n'),
            "{at}: one line"
        );
        assert_eq!(
            h.comp.get_panel_device_button_shown(),
            v.button.is_some(),
            "{at}"
        );
        assert!(
            !h.comp.get_panel_device_controls_shown(),
            "{at}: no device buttons before it is live"
        );
        // The line is drawn in the body (ink in the box), and nothing is drawn as a picture.
        let f = h.render();
        let ink = f.count_where(
            (h.comp.get_panel_device_x() * 1.5) as u32,
            (h.comp.get_panel_device_y() * 1.5) as u32,
            ((h.comp.get_panel_device_x() + h.comp.get_panel_device_w()) * 1.5) as u32,
            ((h.comp.get_panel_device_y() + h.comp.get_panel_device_h()) * 1.5) as u32,
            |p| p[0] > 110 && p[1] > 110 && p[2] > 110,
        );
        assert!(ink > 20, "{at}: its line is drawn ({ink})");
        if v.button.is_some() {
            let before = setups.get();
            let x = h.comp.get_panel_device_button_x() + h.comp.get_panel_device_button_w() / 2.0;
            let y = h.comp.get_panel_device_button_y() + h.comp.get_panel_device_button_h() / 2.0;
            assert!(h.comp.get_panel_device_button_w() > 40.0, "{at}");
            h.click(x, y);
            assert_eq!(setups.get(), before + 1, "{at}: its one button answers");
        }
        if v.state == 2 {
            // The list: a click on the row picks it.
            let (bx, by, bw, bh) = box_of(&h);
            let mid = (bx + bw / 2.0, by + bh / 2.0);
            // The row is under the line, around the middle of the box.
            for dy in [-6.0, 0.0, 6.0, 12.0, 18.0, 24.0, 30.0] {
                h.click(mid.0, mid.1 + dy);
                if picks.get() >= 0 {
                    break;
                }
            }
            assert_eq!(picks.get(), 0, "{at}: the row picks");
        }
    }
}

/// The controller over the fake and two tabs' panels.
struct Rig {
    h: Harness<AppWindow>,
    panels: PanelCtl,
    dev: DeviceCtl,
}

impl Rig {
    fn new(fake: FakeSource) -> Rig {
        let post: crate::appctl::Post = Arc::new(|_| {});
        let mut panels = PanelCtl::new(Arc::clone(&post));
        panels.set_device_offered(true);
        Rig {
            h: window(1.0),
            panels,
            dev: DeviceCtl::new(post, Rc::new(|| 0), Some(fake)),
        }
    }

    fn front(&mut self, tab: &str) {
        let names: HashMap<String, String> = [
            ("t1".into(), "Flare app".into()),
            ("t2".into(), "Notes".into()),
        ]
        .into();
        self.panels.sync(&self.h.comp, Some(tab));
        for e in self.dev.update(&self.panels, Some(tab), false, names) {
            if let Effect::SetTarget { tab, target } = e {
                self.panels.set_device_target(&tab, &target);
            }
        }
        self.dev.push(&self.h.comp);
        self.h.advance(0);
    }
}

#[test]
fn no_xcode_offers_get_xcode() {
    let s = Setup::NoXcode.view(false);
    assert_eq!(s.line, "The device panel needs Xcode.");
    assert_eq!(s.button, Some(SetupButton::GetXcode));
    assert_eq!(SetupButton::GetXcode.label(), "Get Xcode");
    let mut r = Rig::new(FakeSource::default());
    r.dev.set_xcode(Xcode::Missing);
    r.panels.show("t1", PanelKind::Device, TARGET);
    r.front("t1");
    assert_eq!(r.h.comp.get_panel_device_button().as_str(), "Get Xcode");
    assert!(r.h.comp.get_panel_device_button_shown());
    // The button opens Xcode's page in the App Store, nothing else.
    assert_eq!(r.dev.setup_clicked(), [Effect::OpenUrl(XCODE_STORE.into())]);
    assert!(r.dev.fake().unwrap().inputs().is_empty());
    // Xcode not set up asks to open it once.
    r.dev.set_xcode(Xcode::NotSetUp);
    r.front("t1");
    assert_eq!(r.h.comp.get_panel_device_button().as_str(), "Open Xcode");
    assert_eq!(r.dev.setup_clicked(), [Effect::OpenXcode]);
}

#[test]
fn still_mode_shows_the_badge_and_the_reason() {
    let mut r = Rig::new(FakeSource::default());
    r.panels.show("t1", PanelKind::Device, TARGET);
    r.front("t1");
    r.dev.setup_clicked(); // Boot
    r.front("t1");
    assert!(r.h.comp.get_panel_device_has_frame(), "live first");
    assert_eq!(r.h.comp.get_panel_device_still().as_str(), "");
    r.dev
        .fake_mut()
        .unwrap()
        .go_broken("the simulator helper stopped three times");
    r.front("t1");
    let c = &r.h.comp;
    assert!(c.get_panel_device_still_shown());
    assert_eq!(
        c.get_panel_device_still().as_str(),
        "Live view unavailable: the simulator helper stopped three times. Showing a picture every half second."
    );
    assert!(!c.get_panel_device_controls(), "no input while still");
    assert_eq!(
        c.get_panel_device_slow().as_str(),
        "",
        "the still badge, not the slow mark"
    );
    // The badge's dot is drawn at the box's top left.
    let f = r.h.render();
    let (bx, by, ..) = box_of(&r.h);
    assert!(
        count_in(&f, 1.0, (bx, by, 60.0, 40.0), WARN, 24) > 6,
        "the badge's dot"
    );
}

#[test]
fn the_cpu_path_shows_the_slow_mark() {
    let mut r = Rig::new(FakeSource::default());
    r.panels.show("t1", PanelKind::Device, TARGET);
    r.front("t1");
    assert!(
        !r.h.comp.get_panel_device_slow_shown(),
        "no mark before a picture"
    );
    r.dev.setup_clicked();
    r.front("t1");
    let c = &r.h.comp;
    assert!(c.get_panel_device_has_frame());
    assert!(c.get_panel_device_slow_shown());
    assert_eq!(
        c.get_panel_device_slow().as_str(),
        crate::device_ctl::CPU_SLOW
    );
    // The fake's frame colour is on screen, and the mark's word is drawn in the header.
    let f = r.h.render();
    let colour = flare_device::fake::colour(1);
    let pic = (
        c.get_panel_device_pic_x() + 30.0,
        c.get_panel_device_pic_y() + 60.0,
        c.get_panel_device_pic_w() - 60.0,
        c.get_panel_device_pic_h() - 120.0,
    );
    assert!(count_in(&f, 1.0, pic, colour, 8) > 100, "the frame shows");
    let head = (
        c.get_page_left() + c.get_panel_slot_x(),
        c.get_panel_slot_top(),
        c.get_panel_visible_w(),
        c.get_panel_header_h(),
    );
    assert!(
        count_in(&f, 1.0, head, WARN, 60) > 6,
        "the slow mark is drawn"
    );
}

#[test]
fn a_second_tab_on_the_same_device_is_view_only() {
    let mut r = Rig::new(FakeSource::default());
    r.panels.show("t1", PanelKind::Device, TARGET);
    r.panels.show("t2", PanelKind::Device, TARGET);
    r.front("t1");
    r.dev.setup_clicked();
    r.front("t1");
    assert!(r.h.comp.get_panel_device_controls(), "t1 drives it");
    assert_eq!(r.h.comp.get_panel_device_foot().as_str(), "");
    // The second tab shows it too, view only, and says who drives it.
    r.front("t2");
    let c = &r.h.comp;
    assert!(c.get_panel_device_has_frame(), "it shows the picture");
    assert!(!c.get_panel_device_controls(), "no device buttons");
    assert_eq!(
        c.get_panel_device_foot().as_str(),
        "Driven from \"Flare app\". View only here."
    );
    // Its pointer goes nowhere.
    let n = r.dev.fake().unwrap().inputs().len();
    let (_, _, bw, bh) = box_of(&r.h);
    r.dev.page_pointer(&r.h.comp, 0, bw / 2.0, bh / 2.0, 0, 0);
    r.dev.page_pointer(&r.h.comp, 1, bw / 2.0, bh / 2.0, 0, 0);
    r.dev.page_key(true, "a", 0, false);
    let touched = r.dev.fake().unwrap().inputs()[n..]
        .iter()
        .any(|i| matches!(i, Recorded::Touch(..) | Recorded::Key(..)));
    assert!(!touched, "view only");
    // The driving tab's panel closes: the second tab drives it now.
    r.panels.close("t1");
    r.front("t2");
    assert!(
        r.h.comp.get_panel_device_controls(),
        "t2 drives it once t1 lets go"
    );
}

#[test]
fn a_refresh_keeps_the_device_list_rows_under_the_pointer() {
    // The window refreshes often (terminal output, timers): the list's rows must not be made again each time, or a
    // refresh between the press and the release on a row rebuilds its TouchArea and the pick is lost.
    use slint::Model as _;
    let mut r = Rig::new(FakeSource::default());
    r.panels
        .show("t1", PanelKind::Device, crate::device_ctl::CHOOSE);
    r.front("t1");
    assert_eq!(r.h.comp.get_panel_device_state(), 2, "the list");
    let first = r.h.comp.get_panel_device_rows();
    assert!(first.row_count() > 0);
    r.front("t1");
    r.front("t1");
    assert!(
        r.h.comp.get_panel_device_rows() == first,
        "the same rows model after a refresh"
    );
}

/// Android setup's licence screen (spec "Setup", 2; Android plan PR A4): its line, Google's licence text in a box
/// under it, and two buttons, [Accept and set up] and [Cancel], each answering on its own.
#[test]
fn the_android_licence_shows_its_text_and_two_buttons() {
    let s = Setup::Licence {
        text: "Terms and Conditions\n\nThis is the Android Software Development Kit License Agreement.".into(),
        download_mb: 2538,
    };
    let h = window(1.5);
    let setups = Rc::new(Cell::new(0));
    let cancels = Rc::new(Cell::new(0));
    {
        let setups = setups.clone();
        h.comp
            .on_panel_device_setup(move || setups.set(setups.get() + 1));
        let cancels = cancels.clone();
        h.comp
            .on_panel_device_cancel(move || cancels.set(cancels.get() + 1));
    }
    show_setup(&h, &s);
    let v = s.view(false);
    h.comp.set_panel_device_has_cancel(v.cancel);
    if let Setup::Licence { text, .. } = &s {
        h.comp.set_panel_device_detail(text.as_str().into());
    }
    h.advance(0);
    // The licence box moves the buttons down: a render lays it out before their place is read.
    let _ = h.render();
    assert!(h.comp.get_panel_device_button_shown());
    let (bx, by, bw, bh) = (
        h.comp.get_panel_device_button_x(),
        h.comp.get_panel_device_button_y(),
        h.comp.get_panel_device_button_w(),
        h.comp.get_panel_device_button_h(),
    );
    h.click(bx + bw / 2.0, by + bh / 2.0);
    assert_eq!((setups.get(), cancels.get()), (1, 0), "Accept and set up");
    // [Cancel] sits to its right.
    h.click(bx + bw + 8.0 + 20.0, by + bh / 2.0);
    assert_eq!((setups.get(), cancels.get()), (1, 1), "Cancel");
    // The licence text is drawn above the buttons: ink between the line and them.
    let f = h.render();
    let ink = f.count_where(
        (h.comp.get_panel_device_x() * 1.5) as u32,
        ((by - 200.0) * 1.5) as u32,
        ((h.comp.get_panel_device_x() + h.comp.get_panel_device_w()) * 1.5) as u32,
        ((by - 14.0) * 1.5) as u32,
        |p| p[0] > 90 && p[1] > 90 && p[2] > 90,
    );
    assert!(ink > 20, "the licence text is drawn ({ink})");
}

/// The person's input on an Android device (Android PR A7, with the Android fake): a click in the middle of the
/// picture is a touch at the middle of its 411 x 914 dp screen, and a key reaches it as a key.
#[test]
fn an_android_panel_takes_touches_in_dp_and_keys() {
    let mut r = Rig::new(FakeSource::android());
    r.panels.show("t1", PanelKind::Device, "emu/Flare_Pixel");
    r.front("t1");
    r.dev.setup_clicked(); // Boot
    r.front("t1");
    assert!(r.h.comp.get_panel_device_controls(), "t1 drives it");
    let n = r.dev.fake().unwrap().inputs().len();
    let (_, _, bw, bh) = box_of(&r.h);
    r.dev.page_pointer(&r.h.comp, 0, bw / 2.0, bh / 2.0, 0, 0);
    r.dev.page_pointer(&r.h.comp, 1, bw / 2.0, bh / 2.0, 0, 0);
    r.dev.page_key(true, "a", 0, false);
    r.dev.page_key(false, "a", 0, false);
    let got = &r.dev.fake().unwrap().inputs()[n..];
    let touches: Vec<(f64, f64)> = got
        .iter()
        .filter_map(|i| match i {
            Recorded::Touch(t, touch) if t.name() == "emu/Flare_Pixel" => Some((touch.x, touch.y)),
            _ => None,
        })
        .collect();
    assert!(touches.len() >= 2, "{got:?}");
    assert!(
        touches
            .iter()
            .all(|(x, y)| (x - 411.0 / 2.0).abs() <= 2.0 && (y - 914.0 / 2.0).abs() <= 2.0),
        "{touches:?}"
    );
    assert!(
        got.iter()
            .any(|i| matches!(i, Recorded::Key(_, k) if k.down)),
        "{got:?}"
    );
}

/// The spec's fallbacks on an Android panel (Android PR A7): the slow mark with its reason, the still badge with its
/// reason, and the last picture dimmed while the display is off.
#[test]
fn an_android_panel_shows_slow_still_and_asleep() {
    use flare_device::source::{FrameMode, SourceEvent};
    let mut r = Rig::new(FakeSource::android());
    r.panels.show("t1", PanelKind::Device, "emu/Flare_Pixel");
    r.front("t1");
    r.dev.setup_clicked(); // Boot
    r.front("t1");
    let t = flare_device::target::Target::emu("Flare_Pixel").unwrap();
    let mode = |r: &mut Rig, mode: FrameMode, why: &str| {
        r.dev.fake().unwrap().emit(SourceEvent::Mode {
            target: t.clone(),
            mode,
            why: why.into(),
        });
        r.front("t1");
    };
    assert!(!r.h.comp.get_panel_device_dim());
    mode(&mut r, FrameMode::Slow, "the frame stream was refused");
    let slow = r.h.comp.get_panel_device_slow().to_string();
    assert!(
        slow.contains("slow way") && slow.contains("refused"),
        "{slow}"
    );
    assert_eq!(r.h.comp.get_panel_device_still().as_str(), "");
    mode(&mut r, FrameMode::Still, "no frame for 2 s");
    assert!(r.h.comp.get_panel_device_still_shown());
    assert!(
        r.h.comp
            .get_panel_device_still()
            .starts_with("Live view unavailable: no frame for 2 s."),
        "{}",
        r.h.comp.get_panel_device_still()
    );
    mode(&mut r, FrameMode::Asleep, "");
    assert!(
        r.h.comp.get_panel_device_dim(),
        "the display is off: dimmed"
    );
    assert!(
        r.h.comp.get_panel_device_has_frame(),
        "the last picture stays"
    );
    mode(&mut r, FrameMode::Live, "");
    assert!(!r.h.comp.get_panel_device_dim());
    assert_eq!(r.h.comp.get_panel_device_still().as_str(), "");
}

/// Android PR A7 on a Mac (iPhones and Android, Android not set up): the chooser lists the devices and, in the same
/// row style, "Android: set up (about 2.5 GB from Google)"; that row leads to Google's licence with its two buttons,
/// as on Windows.
#[test]
fn a_mac_chooser_has_an_android_setup_row_that_leads_to_the_licence() {
    use crate::device_ctl::{AndroidSdk, Booted, SourceSetup, Started, XcodeAnswer};
    use slint::Model as _;
    let post: crate::appctl::Post = Arc::new(|_| {});
    let mut panels = PanelCtl::new(Arc::clone(&post));
    panels.set_device_offered(true);
    let mut dev = DeviceCtl::new(post, Rc::new(|| 0), None);
    // The sim host's place taken by the iPhone fake, the emu host's by the Android one.
    let mut fake = Some(FakeSource::android());
    let mut iphone = Some(FakeSource::default());
    dev.install_real(
        Box::new(move |s: &SourceSetup| match s {
            SourceSetup::Android => Some(Started {
                source: Box::new(fake.take()?),
                #[cfg(target_os = "macos")]
                ring: None,
            }),
            SourceSetup::Xcode(_) => Some(Started {
                source: Box::new(iphone.take()?),
                #[cfg(target_os = "macos")]
                ring: None,
            }),
        }),
        None,
        Rc::new(|_: &std::path::Path, _: &[String]| {}),
    );
    dev.set_ios(true);
    dev.install_android(Rc::new(|_: &[String]| {}));
    dev.on_xcode(XcodeAnswer {
        xcode: Some(flare_device::xcode::Xcode {
            app: "/Applications/Xcode.app".into(),
            version: "27.2".into(),
        }),
        android: Some(AndroidSdk::Missing),
        booted: Booted::default(),
    });
    dev.set_xcode(Xcode::Ready);
    let mut r = Rig {
        h: window(1.0),
        panels,
        dev,
    };
    r.panels
        .show("t1", PanelKind::Device, crate::device_ctl::CHOOSE);
    r.front("t1");
    r.front("t1");
    assert_eq!(r.h.comp.get_panel_device_state(), 2, "the list");
    let rows = r.h.comp.get_panel_device_rows();
    let names: Vec<String> = (0..rows.row_count())
        .filter_map(|i| rows.row_data(i))
        .map(|d| d.name.to_string())
        .collect();
    assert_eq!(
        names.last().map(String::as_str),
        Some("Android: set up (about 2.5 GB from Google)"),
        "{names:?}"
    );
    r.dev.pick(names.len() as i32 - 1);
    r.front("t1");
    let c = &r.h.comp;
    assert_eq!(c.get_panel_device_state(), 1);
    assert!(
        c.get_panel_device_line().starts_with("Google's licence"),
        "{}",
        c.get_panel_device_line()
    );
    assert_eq!(c.get_panel_device_button().as_str(), "Accept and set up");
    assert!(c.get_panel_device_has_cancel());
}

/// An Android device's panel (rule 1371): the header keeps the iOS buttons, the menu says it is Android's (Back and
/// Recents instead of Shake), and the picture's corners are an Android screen's.
#[test]
fn an_android_panel_marks_its_menu_as_androids() {
    let mut r = Rig::new(FakeSource::android());
    r.panels.show("t1", PanelKind::Device, "emu/Flare_Pixel");
    r.front("t1");
    r.dev.setup_clicked(); // Boot
    r.front("t1");
    let c = &r.h.comp;
    assert!(c.get_panel_device_has_frame(), "live");
    assert!(c.get_panel_device_android());
    assert!(
        c.get_panel_device_controls_shown(),
        "Home, Lock and Rotate, as for iOS"
    );
    assert_eq!(
        c.get_panel_device_radius_pt(),
        crate::device_ctl::RADIUS_DP as f32
    );
    assert_eq!(c.get_panel_device_detail().as_str(), "");
    // Lock sends Power.
    r.dev.act("lock");
    assert!(
        r.dev
            .fake()
            .unwrap()
            .inputs()
            .iter()
            .any(|i| matches!(i, Recorded::Button(_, b) if b.name() == "power"))
    );
}
