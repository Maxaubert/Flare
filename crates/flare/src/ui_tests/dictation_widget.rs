//! Tests of the dictation widget window's card at all eight scalings (M8 Task 26).
//!
//! Parity lines N18 (the window), N19 (the faces), N20 (the level), N23 (clicks) and N32 (the inks). The card is
//! 480 css px wide with a 14 px margin, 10 px above and 14 px below; its height follows the face. Rendering is the
//! headless software renderer; the Win32 half (`win`) is tested on a plain popup window parked off every monitor.

use std::cell::RefCell;
use std::rc::Rc;

use flare_dictation::route::OverlayRow;
use flare_dictation::service::{OverlayState, Tone};
use slint::{Model, ModelRc, VecModel};

use crate::dictation_widget::{
    Bounds, Display, bar_scale, bounds_for, face_of, items_of, live_line, set_face, texts,
    wants_inks, win,
};
use crate::ui_harness::{Frame, Harness, SCALES};
use crate::{DictationWidget, WidgetItem};

/// The component's default inks (the widget's .slint file): pt-default's `Theme.tabs` and `Theme.accent-hi`.
const GROUND: [u8; 4] = [23, 23, 23, 255];
const ACCENT: [u8; 4] = [246, 162, 91, 255];

fn row(id: &str, n: u8, group: Option<&str>) -> OverlayRow {
    OverlayRow {
        id: id.into(),
        n,
        label: format!("tab {n}"),
        folder: "a/b".into(),
        state: None,
        disabled: None,
        lead: false,
        group: group.map(str::to_string),
    }
}

fn pick(rows: Vec<OverlayRow>) -> OverlayState {
    OverlayState::Pick {
        text: "check the build".into(),
        rows,
    }
}

fn listening(text: &str, level: f32) -> OverlayState {
    OverlayState::Listening {
        text: text.into(),
        level,
        starting: false,
    }
}

/// A card for a face, drawn at `scale`, in a window as tall as the face needs.
fn card(scale: f32, state: &OverlayState) -> (Harness<DictationWidget>, Rc<VecModel<WidgetItem>>) {
    let f = face_of(state);
    let h = Harness::new(scale, 480, f.height, || DictationWidget::new().unwrap());
    let model = Rc::new(VecModel::<WidgetItem>::default());
    h.comp.set_items(ModelRc::from(model.clone()));
    set_face(&h.comp, &model, &f);
    h.advance(500);
    (h, model)
}

fn dev(scale: f32, css: f32) -> i64 {
    (css * scale).round() as i64
}

fn close(got: i64, want: i64, tol: i64, what: &str) {
    assert!((got - want).abs() <= tol, "{what}: got {got}, want {want}");
}

fn is_card(p: [u8; 4]) -> bool {
    p[3] >= 240
}

// ---------------------------------------------------------------------------------------------------------------------
// The window and the heights (N18)
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn the_card_is_480_wide_and_the_heights_follow_the_state() {
    let faces = [
        (listening("", 0.0), 196u32),
        (OverlayState::Transcribing, 104),
        (OverlayState::Sending { to: "Web".into() }, 104),
        (OverlayState::Sent { to: "Web".into() }, 104),
        (
            OverlayState::Note {
                message: "Not sent".into(),
                tone: Tone::Quiet,
            },
            104,
        ),
        (pick(vec![row("a", 1, None), row("b", 2, None)]), 196 + 96),
        (
            pick(vec![
                row("a", 1, Some("Flare")),
                row("b", 2, Some("Flare")),
                row("c", 3, Some("Flare (2)")),
            ]),
            196 + 48 * 3 + 30 * 2,
        ),
    ];
    for s in SCALES {
        for (state, want_h) in &faces {
            let (h, _m) = card(s, state);
            assert_eq!(face_of(state).height, *want_h);
            let f = h.render();
            close(f.w as i64, dev(s, 480.0), 0, "window width");
            close(f.h as i64, dev(s, *want_h as f32), 0, "window height");
            // The card in css px, as the component lays it out.
            assert!((h.comp.get_card_x() - 14.0).abs() < 0.01, "scale {s}");
            assert!((h.comp.get_card_y() - 10.0).abs() < 0.01, "scale {s}");
            assert!((h.comp.get_card_w() - 452.0).abs() < 0.01, "scale {s}");
            assert!(
                (h.comp.get_card_h() - (*want_h as f32 - 24.0)).abs() < 0.01,
                "scale {s}"
            );
            // And as pixels: 14 in from each side, 10 down from the top, 14 up from the foot.
            let (x0, y0, x1, y1) = f
                .bbox_where(0, 0, f.w, f.h, is_card)
                .unwrap_or_else(|| panic!("scale {s}: no card drawn"));
            close(x0 as i64, dev(s, 14.0), 1, "card left");
            close(y0 as i64, dev(s, 10.0), 1, "card top");
            close((x1 + 1) as i64, dev(s, 466.0), 1, "card right");
            close(
                (y1 + 1) as i64,
                dev(s, *want_h as f32 - 14.0),
                1,
                "card foot",
            );
            // Transparent outside the card (the glass surface keeps its alpha).
            assert_eq!(f.px(0, 0)[3], 0, "scale {s}: the corner is not clear");
        }
    }
}

#[test]
fn a_flat_surface_paints_the_ground_behind_the_card() {
    for s in [1.0_f32, 2.25] {
        let (h, _m) = card(s, &listening("", 0.0));
        h.comp.set_flat(true);
        h.advance(0);
        let f = h.render();
        assert!(
            Frame::near(f.px(0, 0), GROUND, 2),
            "scale {s}: {:?}",
            f.px(0, 0)
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The faces (N19) and the level (N20)
// ---------------------------------------------------------------------------------------------------------------------

/// The accent fill of the meter, as a fraction of the track, read from the pixels of the meter's middle row.
fn meter_fill(f: &Frame, s: f32) -> f32 {
    let y = dev(s, 10.0 + 60.0 + 2.0) as u32;
    let x0 = dev(s, 14.0 + 16.0) as u32;
    let run = f.run_len_h(y, x0 + 1, |p| Frame::near(p, ACCENT, 24)) + 1;
    run as f32 / (420.0 * s)
}

#[test]
fn the_level_bar_follows_max_0_04_min_1_level_x3() {
    // The rule itself.
    assert_eq!(bar_scale(0.0), 0.04);
    assert_eq!(bar_scale(0.01), 0.04);
    assert!((bar_scale(0.1) - 0.3).abs() < 1e-6);
    assert!((bar_scale(0.2) - 0.6).abs() < 1e-6);
    assert_eq!(bar_scale(0.5), 1.0);
    assert_eq!(bar_scale(7.0), 1.0);
    assert_eq!(bar_scale(f32::NAN), 0.04);
    assert_eq!(bar_scale(-1.0), 0.04);
    // The card draws it: the accent run along the 420 css px track.
    for s in SCALES {
        for level in [0.0_f32, 0.1, 0.2, 0.9] {
            let (h, _m) = card(s, &listening("hello", level));
            let got = meter_fill(&h.render(), s);
            let want = bar_scale(level);
            assert!(
                (got - want).abs() <= 3.0 / (420.0 * s) + 0.005,
                "scale {s}, level {level}: the bar is {got}, want {want}"
            );
        }
    }
}

#[test]
fn a_level_update_redraws_the_bar_only() {
    let s = 2.25;
    let (h, _m) = card(s, &listening("hello", 0.0));
    let before = h.render();
    h.comp.set_bar(bar_scale(0.2));
    h.advance(200);
    let after = h.render();
    // Everything above the meter's row and below it is the same pixel for pixel.
    let meter_top = dev(s, 10.0 + 60.0) as u32;
    let meter_foot = dev(s, 10.0 + 60.0 + 4.0) as u32 + 1;
    for y in (0..before.h).filter(|y| *y < meter_top || *y > meter_foot) {
        for x in 0..before.w {
            assert_eq!(before.px(x, y), after.px(x, y), "({x}, {y})");
        }
    }
    assert_ne!(
        meter_fill(&before, s),
        meter_fill(&after, s),
        "the bar did not move"
    );
}

#[test]
fn texts_are_pts() {
    assert_eq!(texts::LISTENING, "Listening");
    assert_eq!(texts::TRANSCRIBING, "Transcribing");
    assert_eq!(texts::PICK_TITLE, "Send to");
    assert_eq!(texts::LISTENING_HINT, "Let go to finish. Esc cancels.");
    assert_eq!(
        texts::PICK_HINT,
        "Press a number or click. Esc keeps it unsent."
    );
    assert_eq!(live_line("", true), "Starting the speech engine");
    assert_eq!(live_line("", false), "Speak now");
    assert_eq!(live_line("hello", true), "hello");
    let titles = |s: OverlayState| face_of(&s).title;
    assert_eq!(
        titles(OverlayState::Sending { to: "Web".into() }),
        "Sending to Web"
    );
    assert_eq!(
        titles(OverlayState::Sent { to: "Web".into() }),
        "Sent to Web"
    );
    let note = |tone| OverlayState::Note {
        message: "Not sent".into(),
        tone,
    };
    assert_eq!(titles(note(Tone::Quiet)), "Not sent");
    assert!(face_of(&note(Tone::Warn)).warn);
    assert!(!face_of(&note(Tone::Quiet)).warn);
    // The .slint file says the same words (it cannot read them from here).
    let slint = include_str!("../../ui/dictation_widget.slint");
    for w in [
        texts::LISTENING_HINT,
        texts::PICK_HINT,
        texts::STARTING,
        texts::SPEAK_NOW,
    ] {
        assert!(slint.contains(w), "the card's words changed: {w}");
    }
    // What the card shows is what the face says.
    let (h, _m) = card(1.0, &OverlayState::Sent { to: "Web".into() });
    assert_eq!(h.comp.get_head(), "Sent to Web");
    let (h, _m) = card(1.0, &listening("", 0.0));
    assert_eq!(h.comp.get_head(), "Listening");
    assert_eq!(h.comp.get_words(), "");
}

// ---------------------------------------------------------------------------------------------------------------------
// The picker (N19, N23)
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn pick_rows_are_numbered_with_headers_only_for_two_copies() {
    // One copy: no group on any row, no header.
    let one = items_of(&[row("a", 1, None), row("b", 2, None)]);
    assert_eq!(one.len(), 2);
    assert!(one.iter().all(|i| !i.header));
    assert_eq!((one[0].n, one[1].n), (1, 2));
    // Two copies: a header before each group, numbers run on across the groups.
    let rows = vec![
        row("a", 1, Some("Flare")),
        row("b", 2, Some("Flare")),
        row("c", 3, Some("Flare (2)")),
    ];
    let two = items_of(&rows);
    let shape: Vec<(bool, &str)> = two.iter().map(|i| (i.header, i.label.as_str())).collect();
    assert_eq!(
        shape,
        vec![
            (true, "Flare"),
            (false, "tab 1"),
            (false, "tab 2"),
            (true, "Flare (2)"),
            (false, "tab 3"),
        ]
    );
    assert_eq!(
        two.iter()
            .filter(|i| !i.header)
            .map(|i| i.n)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    // The window height counts the same rows and headers (N18).
    assert_eq!(face_of(&pick(rows)).height, 196 + 48 * 3 + 30 * 2);
    // The card draws a header per group and a number per row: the model it was given has them.
    let (h, m) = card(
        1.0,
        &pick(vec![
            row("a", 1, Some("Flare")),
            row("c", 2, Some("Flare (2)")),
        ]),
    );
    assert_eq!(m.row_count(), 4);
    assert_eq!(h.comp.get_head(), "Send to");
}

#[test]
fn a_disabled_row_has_its_title() {
    let mut asking = row("a", 1, None);
    asking.disabled = Some("Waiting on a question: answer it there first".into());
    let mut working = row("b", 2, None);
    working.state = Some("working".into());
    let items = items_of(&[asking, working, row("c", 3, None)]);
    assert!(items[0].disabled);
    assert_eq!(
        items[0].reason,
        "Waiting on a question: answer it there first"
    );
    assert_eq!(items[0].chip, "asking");
    assert!(!items[1].disabled);
    assert_eq!(items[1].reason, "");
    assert_eq!(items[1].chip, "working");
    assert_eq!(items[2].chip, "");
    let mut lead = row("d", 4, None);
    lead.lead = true;
    assert_eq!(items_of(&[lead])[0].chip, "orchestrator");
}

/// The window y of the middle of item `i` (css px): the card's 10, the rows' 116, a 4 gap after each item.
fn item_mid(items: &[crate::dictation_widget::Item], i: usize) -> f32 {
    let above: f32 = items[..i]
        .iter()
        .map(|it| if it.header { 26.0 } else { 44.0 } + 4.0)
        .sum();
    let own = if items[i].header { 26.0 } else { 44.0 };
    10.0 + 116.0 + above + own / 2.0
}

#[test]
fn a_click_picks_a_row_and_a_disabled_row_cannot_be_picked() {
    for s in SCALES {
        let mut asking = row("a", 1, Some("Flare"));
        asking.disabled = Some("Waiting on a question: answer it there first".into());
        let rows = vec![
            asking,
            row("b", 2, Some("Flare")),
            row("c", 3, Some("Flare (2)")),
        ];
        let items = items_of(&rows);
        let (h, _m) = card(s, &pick(rows));
        let picked = Rc::new(RefCell::new(Vec::<String>::new()));
        let sink = Rc::clone(&picked);
        h.comp
            .on_pick(move |id| sink.borrow_mut().push(id.to_string()));
        // [header, a (disabled), b, header, c]
        h.click(180.0, item_mid(&items, 1));
        assert!(
            picked.borrow().is_empty(),
            "scale {s}: a disabled row was picked"
        );
        h.click(180.0, item_mid(&items, 0));
        assert!(picked.borrow().is_empty(), "scale {s}: a header was picked");
        h.click(180.0, item_mid(&items, 2));
        assert_eq!(*picked.borrow(), ["b"], "scale {s}");
        h.click(180.0, item_mid(&items, 4));
        assert_eq!(*picked.borrow(), ["b", "c"], "scale {s}");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The icons (size tests for the stroked glyph)
// ---------------------------------------------------------------------------------------------------------------------

/// The accent-coloured bounding box (css px) inside the 19 px icon box at css (37.5, `icon_y`), slightly widened.
fn glyph_box(f: &Frame, s: f32, icon_y: f32) -> (f32, f32, f32, f32) {
    let x0 = dev(s, 37.5 - 3.0) as u32;
    let x1 = dev(s, 37.5 + 19.0 + 3.0) as u32;
    let y0 = dev(s, icon_y - 3.0) as u32;
    let y1 = dev(s, icon_y + 19.0 + 3.0) as u32;
    let (a, b, c, d) = f
        .bbox_where(x0, y0, x1, y1, |p| Frame::near(p, ACCENT, 60))
        .expect("the glyph is drawn");
    (
        a as f32 / s,
        b as f32 / s,
        (c + 1) as f32 / s,
        (d + 1) as f32 / s,
    )
}

fn assert_box(got: (f32, f32, f32, f32), want: (f32, f32, f32, f32), s: f32, what: &str) {
    let tol = 1.2 + 1.5 / s;
    for (g, w) in [got.0, got.1, got.2, got.3]
        .iter()
        .zip([want.0, want.1, want.2, want.3])
    {
        assert!(
            (g - w).abs() <= tol,
            "scale {s}, {what}: got {got:?}, want {want:?}"
        );
    }
}

#[test]
fn the_mic_and_the_check_are_the_size_the_glyph_says() {
    // 19 px icon in a 24-unit box: unit = 19/24 px. The mic spans x 6..18, y 3.5..20.5 with a 1.9 stroke; the check
    // spans x 6..18, y 7.5..16.5 with a 2.6 stroke. The icon box sits at window (37.5, 31.5) with the head at 14, and
    // at (37.5, 40.5) when the head is centred in an 80 px card (23).
    let u = 19.0_f32 / 24.0;
    for s in SCALES {
        let (h, _m) = card(s, &listening("", 0.0));
        let got = glyph_box(&h.render(), s, 31.5);
        let want = (
            37.5 + (6.0 - 0.95) * u,
            31.5 + (3.5 - 0.95) * u,
            37.5 + (18.0 + 0.95) * u,
            31.5 + (20.5 + 0.95) * u,
        );
        assert_box(got, want, s, "mic");
        let (h, _m) = card(s, &OverlayState::Sent { to: "x".into() });
        let got = glyph_box(&h.render(), s, 40.5);
        let want = (
            37.5 + (6.0 - 1.3) * u,
            40.5 + (7.5 - 1.3) * u,
            37.5 + (18.0 + 1.3) * u,
            40.5 + (16.5 + 1.3) * u,
        );
        assert_box(got, want, s, "check");
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The inks (N32) and where it sits (N18)
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn the_inks_are_handed_over_at_each_outside_start_only() {
    let transcribing = face_of(&OverlayState::Transcribing);
    let l = |t: &str| face_of(&listening(t, 0.0));
    // Hidden: always.
    assert!(wants_inks(false, None, &l("")));
    assert!(wants_inks(false, None, &transcribing));
    // A new start while the last outcome still shows.
    let sent = face_of(&OverlayState::Sent { to: "x".into() });
    assert!(wants_inks(true, Some(&sent), &l("")));
    // Words arriving on the listening card are not a start.
    assert!(!wants_inks(true, Some(&l("")), &l("hi")));
    assert!(!wants_inks(true, Some(&l("")), &transcribing));
}

#[test]
fn the_card_wears_the_inks_it_is_given() {
    let (h, _m) = card(1.0, &listening("", 0.0));
    h.comp.set_ground(slint::Color::from_rgb_u8(250, 250, 250));
    h.advance(0);
    let f = h.render();
    // The card's fill follows the ground it was handed (alpha 0.96 over clear).
    let p = f.px(dev(1.0, 30.0) as u32, dev(1.0, 150.0) as u32);
    assert!(Frame::near(p, [250, 250, 250, 245], 6), "{p:?}");
}

#[test]
fn the_bounds_follow_the_scale_of_the_monitor() {
    let scales = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0_f64];
    for s in scales {
        // A second monitor to the right of the first, at its own origin, with a taskbar at the foot.
        let work = Bounds {
            x: 2560,
            y: 0,
            w: 3840,
            h: 2100,
        };
        for css_h in [104u32, 196, 196 + 48 * 3 + 30 * 2] {
            let b = bounds_for(work, s, css_h);
            assert_eq!(b.w, (480.0 * s).round() as i32, "scale {s}");
            assert_eq!(b.h, (f64::from(css_h) * s).round() as i32, "scale {s}");
            let left = b.x - work.x;
            let right = work.x + work.w - (b.x + b.w);
            assert!((left - right).abs() <= 1, "scale {s}: {left} {right}");
            assert_eq!(
                work.y + work.h - (b.y + b.h),
                (16.0 * s).round() as i32,
                "scale {s}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Never takes focus (N18)
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn the_style_is_never_active_off_the_taskbar_and_on_top() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    };
    let s = win::widget_ex_style(WS_EX_APPWINDOW);
    assert_ne!(s & WS_EX_NOACTIVATE, 0);
    assert_ne!(s & WS_EX_TOOLWINDOW, 0);
    assert_ne!(s & WS_EX_TOPMOST, 0);
    assert_eq!(s & WS_EX_APPWINDOW, 0);
}

/// Makes a plain popup Win32 window at the parking spot: no monitor ever shows it.
fn parked_popup() -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WS_POPUP};
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let (x, y) = flare_win::window_park::parked_origin();
    // SAFETY: creates a window of the system's STATIC class on this thread, at the parking spot; the test destroys
    // it before it ends.
    let h = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            x,
            y,
            480,
            200,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    assert!(!h.is_null(), "the popup window is made");
    h as isize
}

/// Drives the real widget (`dictation_widget::set`) against a stand-in popup whose handle appears only after a few
/// timer ticks, as the winit window does (it is made in the event loop, after `show()`).
struct Rig {
    popup: isize,
    polls: Rc<std::cell::Cell<u32>>,
}

impl Rig {
    fn new(late_by: u32, displays: Rc<RefCell<Display>>) -> Rig {
        let popup = parked_popup();
        let polls = Rc::new(std::cell::Cell::new(0u32));
        let p = Rc::clone(&polls);
        crate::dictation_widget::test_sources(
            move |_| {
                p.set(p.get() + 1);
                (p.get() > late_by).then_some(popup)
            },
            move || Some(*displays.borrow()),
        );
        crate::ui_harness::provide_window(480, 196);
        Rig { popup, polls }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        crate::dictation_widget::close();
        // SAFETY: destroys the window this rig made, on the thread that made it.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(self.popup as _);
        }
    }
}

fn monitor(x: i32, scale: f64) -> Display {
    Display {
        work: Bounds {
            x,
            y: 0,
            w: (1920.0 * scale) as i32,
            h: (1040.0 * scale) as i32,
        },
        scale,
    }
}

fn show(state: &OverlayState) {
    crate::dictation_widget::set(Some(state), &|| None, true);
}

#[test]
fn the_window_never_takes_focus() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    };
    let displays = Rc::new(RefCell::new(monitor(0, 1.0)));
    let rig = Rig::new(3, displays);
    // SAFETY: reads the foreground window handle.
    let fg_before = unsafe { GetForegroundWindow() } as isize;
    show(&listening("", 0.0));
    // The window has no handle yet: the widget exists, wants to show, and waits.
    let s = crate::dictation_widget::snapshot();
    assert!(s.exists && !s.visible, "{s:?}");
    // The event loop makes the window a few ticks later; the widget picks it up and shows it without being asked again.
    for _ in 0..10 {
        crate::ui_harness::advance_clock(10);
    }
    assert!(rig.polls.get() > 3, "the handle was never fetched again");
    let s = crate::dictation_widget::snapshot();
    assert!(s.visible, "the card never appeared: {s:?}");
    assert!(!s.focusable && s.topmost, "{s:?}");
    let ex = win::ex_style(rig.popup);
    assert_ne!(
        ex & WS_EX_TRANSPARENT,
        0,
        "clicks must pass while listening"
    );
    // A pick face catches clicks, and the never-activate bits survive the toggle.
    show(&pick(vec![row("a", 1, None), row("b", 2, None)]));
    let ex = win::ex_style(rig.popup);
    assert_eq!(ex & WS_EX_TRANSPARENT, 0, "the picker must catch clicks");
    for bit in [WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST] {
        assert_ne!(ex & bit, 0, "style bit {bit:#x} lost on the pick toggle");
    }
    // Something rewrote the style behind our back (winit does on a flag change): the next update puts it back.
    // SAFETY: sets the extended style of the rig's own window.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
            rig.popup as _,
            windows_sys::Win32::UI::WindowsAndMessaging::GWL_EXSTYLE,
            0,
        );
    }
    show(&pick(vec![row("a", 1, None)]));
    assert_ne!(win::ex_style(rig.popup) & WS_EX_NOACTIVATE, 0);
    // SAFETY: reads the foreground window handle.
    let fg_after = unsafe { GetForegroundWindow() } as isize;
    assert_eq!(fg_before, fg_after, "the widget changed the foreground");
    assert_ne!(fg_after, rig.popup);
    crate::dictation_widget::hide();
    assert!(!win::is_visible(rig.popup));
    assert!(!crate::dictation_widget::snapshot().visible);
}

#[test]
fn a_height_change_keeps_the_monitor_it_opened_on() {
    let displays = Rc::new(RefCell::new(monitor(0, 1.0)));
    let rig = Rig::new(0, Rc::clone(&displays));
    show(&listening("", 0.0));
    let first = crate::dictation_widget::snapshot().bounds;
    assert_eq!(first, bounds_for(monitor(0, 1.0).work, 1.0, 196));
    // The pointer goes to a second monitor at another scale; the card grows for the picker.
    *displays.borrow_mut() = monitor(1920, 2.25);
    show(&pick(vec![row("a", 1, None), row("b", 2, None)]));
    let grown = crate::dictation_widget::snapshot().bounds;
    assert_eq!(
        grown,
        bounds_for(monitor(0, 1.0).work, 1.0, 196 + 96),
        "the card jumped monitors in the middle of a session"
    );
    // The next show goes where the pointer is.
    crate::dictation_widget::hide();
    show(&listening("", 0.0));
    let again = crate::dictation_widget::snapshot().bounds;
    assert_eq!(again, bounds_for(monitor(1920, 2.25).work, 2.25, 196));
    drop(rig);
}
#[test]
fn the_heard_words_have_their_bar_on_the_left() {
    for s in SCALES {
        let (h, _m) = card(s, &pick(vec![row("a", 1, None)]));
        let f = h.render();
        // The 1 px bar stands at the left edge of the words' box (card 14 + pad 16), 46 css px tall from y 70.
        let y = dev(s, 10.0 + 60.0 + 23.0) as u32;
        let x0 = dev(s, 14.0 + 16.0) as u32;
        let near = |p: [u8; 4]| Frame::near(p, ACCENT, 80) && p[0] > 120;
        let on_left = f.count_where(
            x0.saturating_sub(2),
            y,
            x0 + dev(s, 2.0) as u32 + 1,
            y + 1,
            near,
        );
        assert!(on_left > 0, "scale {s}: no bar at the left edge");
        let mid = f.count_where(dev(s, 200.0) as u32, y, dev(s, 260.0) as u32, y + 1, near);
        assert_eq!(mid, 0, "scale {s}: a bar stands in the middle");
    }
}
