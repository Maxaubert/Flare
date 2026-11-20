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
