//! The panel splitter, dragged the way a hand does (Surfaces, 2026-10-22): many steps both ways, quick and slow, past
//! both limits, over a picture and over a text file, at 100% and 225%, through the real controller. After every step
//! the window is checked against what must always hold: the slot and the splitter inside their limits, the splitter on
//! the slot's left edge, no gap or overlap between the grid and the slot once the drag is over, the panel's content
//! drawn, the grid held (no refit) while the drag lasts and refitted exactly once on the release, the slot following
//! the pointer without jitter, and the grab let go when the pointer leaves the window.

use slint::platform::PointerEventButton;

use super::*;

const LEFT: PointerEventButton = PointerEventButton::Left;

/// What one step of a drag showed.
#[derive(Clone, Debug)]
struct Seen {
    pointer_x: f32,
    visible_w: f32,
    splitter_mid: f32,
    slot_left: f32,
    grid_w: i32,
    splitting: bool,
}

fn seen(f: &Fx, pointer_x: f32) -> Seen {
    let c = &f.h.comp;
    Seen {
        pointer_x,
        visible_w: c.get_panel_visible_w(),
        splitter_mid: c.get_panel_splitter_x() + c.get_panel_splitter_w() / 2.0,
        slot_left: c.get_grid_left() + c.get_panel_slot_x(),
        grid_w: c.get_grid_phys_width(),
        splitting: c.get_panel_splitting(),
    }
}

fn start_point(f: &Fx) -> (f32, f32) {
    let c = &f.h.comp;
    (
        c.get_panel_splitter_x() + c.get_panel_splitter_w() / 2.0,
        c.get_grid_top() + 200.0,
    )
}

/// Ink of the panel's content in the frame: pixels that are neither the panel's ground nor its header.
fn content_ink(f: &Fx, frame: &crate::ui_harness::Frame, kind: i32) -> usize {
    let s = f.h.scale();
    let c = &f.h.comp;
    let x0 = (c.get_panel_body_x() * s).round() as u32 + 2;
    let y0 = (c.get_panel_body_y() * s).round() as u32 + 2;
    let x1 = ((c.get_panel_body_x() + c.get_panel_body_w()) * s).round() as u32;
    let y1 = ((c.get_panel_body_y() + c.get_panel_body_h()) * s).round() as u32;
    frame.count_where(x0, y0, x1, y1, |p| {
        if kind == 1 {
            // The picture's own colour (see `write_png`: 200, 100, 50).
            Frame::near(p, [200, 100, 50, 255], 10)
        } else {
            p[0] > 90 && p[1] > 90 && p[2] > 90
        }
    })
}

use crate::ui_harness::Frame;

/// Drags the splitter along `xs` (pointer positions, in css px, relative to the start), checking every step.
/// Returns what went wrong.
fn drag(f: &Fx, fits: &Rc<Cell<u32>>, kind: i32, xs: &[f32], slow: bool) -> Vec<String> {
    let mut bad = Vec::new();
    let c = &f.h.comp;
    let s = f.h.scale();
    let (x, y) = start_point(f);
    let page_w = c.get_page_w();
    let (min_phys, max_phys, _) =
        flare_surfaces::layout::limits((page_w * s).round() as i32, f64::from(s));
    let (min, max) = (min_phys as f32 / s, max_phys as f32 / s);
    let w0 = c.get_panel_slot_w();
    let grid0 = c.get_grid_phys_width();
    let fits0 = fits.get();
    f.h.move_to(x, y);
    f.h.press(x, y, LEFT);
    f.h.advance(0);
    if !c.get_panel_splitting() {
        bad.push("the press did not start a drag".to_string());
        return bad;
    }
    let mut last_vis = w0;
    for (n, dx) in xs.iter().enumerate() {
        let px = x + dx;
        f.h.move_to(px, y);
        if slow {
            f.h.advance(16);
        } else {
            f.h.advance(0);
        }
        f.pump();
        let v = seen(f, px);
        let at = format!("step {n} (dx {dx}, scale {s})");
        // The slot and the splitter stay inside their limits.
        if v.visible_w < min - 1.01 / s || v.visible_w > max + 1.01 / s {
            bad.push(format!("{at}: slot {} outside [{min}, {max}]", v.visible_w));
        }
        // The splitter sits on the slot's left edge.
        if (v.splitter_mid - v.slot_left).abs() > 1.01 {
            bad.push(format!(
                "{at}: splitter at {} but the slot starts at {}",
                v.splitter_mid, v.slot_left
            ));
        }
        // The slot follows the pointer exactly (within a device pixel), clamped to the limits.
        let want = (w0 - dx).clamp(min, max);
        if (v.visible_w - want).abs() > 1.01 / s {
            bad.push(format!(
                "{at}: slot {} but the pointer says {want}",
                v.visible_w
            ));
        }
        // Moving the pointer one way never moves the slot the other way (a slot that wobbles under a steady hand is
        // jitter: it would show here as a step against the pointer).
        let d = v.visible_w - last_vis;
        let pointer_dir = if n == 0 { 0.0 } else { dx - xs[n - 1] };
        if pointer_dir != 0.0 && d * pointer_dir > 0.5 {
            bad.push(format!(
                "{at}: pointer moved {pointer_dir} but the slot moved {d}"
            ));
        }
        last_vis = v.visible_w;
        // The grid holds while the drag lasts (rule 24): no refit, no change of width.
        if v.grid_w != grid0 {
            bad.push(format!(
                "{at}: the grid changed width ({grid0} to {})",
                v.grid_w
            ));
        }
        if fits.get() != fits0 {
            bad.push(format!("{at}: the grid was fitted during the drag"));
        }
        if !v.splitting {
            bad.push(format!("{at}: the drag ended by itself"));
        }
        // The panel's content is drawn on every frame.
        let frame = f.h.render();
        let ink = content_ink(f, &frame, kind);
        if ink < 40 {
            bad.push(format!(
                "{at}: the panel's content is not drawn ({ink} inked pixels)"
            ));
        }
        // Narrower than it began, the grid (held at its width) ends short of the slot: what shows in the gap must be
        // the terminal's own ground, not the page's.
        let gap_from = (c.get_grid_left() * s).round() as i32 + grid0;
        let gap_to = (v.slot_left * s).round() as i32;
        if gap_to - gap_from > 2 {
            let y = ((c.get_grid_top() + 420.0) * s).round() as u32;
            let beside = frame.px(((c.get_grid_left() * s).round() as u32) + 6, y);
            for gx in [gap_from + 1, (gap_from + gap_to) / 2, gap_to - 2] {
                let g = frame.px(gx.max(0) as u32, y);
                if !Frame::near(g, beside, 3) {
                    bad.push(format!(
                        "{at}: the gap beside the slot shows {g:?} but the terminal's ground is {beside:?}"
                    ));
                    break;
                }
            }
        }
    }
    // Let go.
    let end = x + xs.last().copied().unwrap_or(0.0);
    f.h.release(end, y, LEFT);
    f.h.advance(0);
    f.pump();
    let _ = page_w;
    if c.get_panel_splitting() {
        bad.push("the release did not end the drag".to_string());
    }
    let settled = c.get_panel_slot_w();
    let want = (w0 - xs.last().copied().unwrap_or(0.0)).clamp(min, max);
    if (settled - want).abs() > 1.01 / s {
        bad.push(format!(
            "after the release the slot is {settled}, the pointer said {want}"
        ));
    }
    if (c.get_panel_visible_w() - settled).abs() > 0.01 {
        bad.push(format!(
            "after the release the drawn slot {} differs from the settled one {settled}",
            c.get_panel_visible_w()
        ));
    }
    // The grid refit exactly once, to the page less the slot.
    let moved = (settled - w0).abs() > 0.5;
    let fitted = fits.get() - fits0;
    if moved && fitted != 1 {
        bad.push(format!(
            "the release fitted the grid {fitted} times, not once"
        ));
    }
    if !moved && fitted != 0 {
        bad.push(format!(
            "nothing moved but the grid was fitted {fitted} times"
        ));
    }
    let frame = f.h.render();
    let ink = content_ink(f, &frame, kind);
    if ink < 40 {
        bad.push(format!(
            "after the release the panel's content is not drawn ({ink})"
        ));
    }
    // No gap and no overlap between the grid and the slot once settled.
    let grid_right = (c.get_grid_left() * s).round() as i32 + c.get_grid_phys_width();
    let slot_left = ((c.get_grid_left() + c.get_panel_slot_x()) * s).round() as i32;
    if (grid_right - slot_left).abs() > 1 {
        bad.push(format!(
            "after the release the grid ends at {grid_right} and the slot starts at {slot_left}"
        ));
    }
    bad
}

fn open_panel(f: &Fx, kind: i32) {
    let id = f.ctl.borrow().front_id().unwrap();
    if kind == 1 {
        let path = f.dir.path().join("p.png");
        write_png(&path, 640, 480);
        f.ctl.borrow_mut().show_preview(
            &id,
            flare_surfaces::PanelKind::Image,
            &path.to_string_lossy(),
        );
        f.pump_until("the picture", |f| f.h.comp.get_panel_kind() == 1);
    } else {
        let body: String = (0..400)
            .map(|i| {
                format!(
                    "line {i} {}
",
                    "text ".repeat(30)
                )
            })
            .collect();
        show_text(f, "a.txt", &body);
        f.pump_until("the text", |f| f.h.comp.get_panel_kind() == 2);
    }
    f.h.advance(0);
}

/// Counts the grid's fits (the window's `grid-resized`; the fixture binds none of its own).
fn count_fits(f: &Fx) -> Rc<Cell<u32>> {
    let n = Rc::new(Cell::new(0u32));
    let c = n.clone();
    // untimed: a test counter
    f.h.comp.on_grid_resized(move |_, _, _| c.set(c.get() + 1));
    f.h.advance(0);
    n
}

fn paths() -> Vec<(&'static str, Vec<f32>, bool)> {
    let line = |a: f32, b: f32, n: usize| -> Vec<f32> {
        (0..=n).map(|i| a + (b - a) * i as f32 / n as f32).collect()
    };
    vec![
        // Wider, in one long slow pull.
        ("wider slow", line(0.0, -150.0, 60), true),
        // Narrower, quick: a few big steps.
        ("narrower quick", line(0.0, 180.0, 6), false),
        // Past the widest limit and back.
        (
            "past max and back",
            [line(0.0, -2000.0, 12), line(-2000.0, -40.0, 12)].concat(),
            false,
        ),
        // Past the narrowest limit and back.
        (
            "past min and back",
            [line(0.0, 2000.0, 12), line(2000.0, 30.0, 12)].concat(),
            true,
        ),
        // A shaky hand: back and forth by a pixel or two.
        (
            "shaky",
            (0..80)
                .map(|i| -60.0 + if i % 2 == 0 { 0.0 } else { 1.0 } - (i as f32) * 0.5)
                .collect(),
            true,
        ),
        // No movement at all.
        ("press and release", vec![0.0, 0.0], false),
    ]
}

fn run(scale: f32, kind: i32) {
    for (name, xs, slow) in paths() {
        let (f, _h, _) = live(Opts {
            scale,
            ..Opts::default()
        });
        open_panel(&f, kind);
        let fits = count_fits(&f);
        let bad = drag(&f, &fits, kind, &xs, slow);
        assert!(
            bad.is_empty(),
            "scale {scale}, kind {kind}, path {name}:\n{}",
            bad.join("\n")
        );
    }
}

#[test]
fn dragging_the_splitter_over_a_picture_at_100_percent_keeps_every_invariant() {
    run(1.0, 1);
}

#[test]
fn dragging_the_splitter_over_a_picture_at_225_percent_keeps_every_invariant() {
    run(2.25, 1);
}

#[test]
fn dragging_the_splitter_over_a_text_file_at_100_percent_keeps_every_invariant() {
    run(1.0, 2);
}

#[test]
fn dragging_the_splitter_over_a_text_file_at_225_percent_keeps_every_invariant() {
    run(2.25, 2);
}

#[test]
fn leaving_the_window_during_a_drag_lets_go_of_the_splitter() {
    for scale in [1.0f32, 2.25] {
        let (f, _h, _) = live(Opts {
            scale,
            ..Opts::default()
        });
        open_panel(&f, 2);
        let fits = count_fits(&f);
        let c = &f.h.comp;
        let (x, y) = start_point(&f);
        let w0 = c.get_panel_slot_w();
        f.h.move_to(x, y);
        f.h.press(x, y, LEFT);
        f.h.move_to(x - 60.0, y);
        f.h.advance(0);
        assert!(c.get_panel_splitting(), "scale {scale}");
        let fits0 = fits.get();
        f.h.leave();
        f.h.advance(0);
        f.pump();
        assert!(
            !c.get_panel_splitting(),
            "scale {scale}: the pointer left, the grab is let go"
        );
        // What was dragged is kept (the release stored it), the grid refit once, and a later move drags nothing.
        assert!(
            (c.get_panel_slot_w() - (w0 + 60.0)).abs() <= 1.5,
            "scale {scale}: {}",
            c.get_panel_slot_w()
        );
        assert_eq!(fits.get(), fits0 + 1, "scale {scale}: one fit");
        let settled = c.get_panel_slot_w();
        f.h.move_to(x - 200.0, y);
        f.h.advance(0);
        assert_eq!(
            c.get_panel_slot_w(),
            settled,
            "scale {scale}: a move after the exit drags nothing"
        );
        assert!(!c.get_panel_splitting());
        f.h.release(x - 200.0, y, LEFT);
        f.h.advance(0);
        assert_eq!(
            c.get_panel_slot_w(),
            settled,
            "scale {scale}: a late release changes nothing"
        );
    }
}

#[test]
fn the_slot_going_away_during_a_drag_lets_go_of_the_grab() {
    for scale in [1.0f32, 2.25] {
        let (f, _h, _) = live(Opts {
            scale,
            ..Opts::default()
        });
        open_panel(&f, 2);
        let c = &f.h.comp;
        let settled = c.get_panel_slot_w();
        let (x, y) = start_point(&f);
        f.h.move_to(x, y);
        f.h.press(x, y, LEFT);
        f.h.move_to(x - 120.0, y);
        f.h.advance(0);
        assert!(c.get_panel_splitting(), "scale {scale}");
        // The Settings page comes to the front: no slot, so no touch area to hear the release.
        f.ctl.borrow_mut().toggle_settings();
        f.pump();
        f.h.advance(0);
        assert!(
            !c.get_panel_splitting(),
            "scale {scale}: the grab is let go"
        );
        f.ctl.borrow_mut().toggle_settings();
        f.pump();
        f.h.advance(0);
        assert!(
            (c.get_panel_visible_w() - settled).abs() <= 1.0,
            "scale {scale}: the slot comes back at its settled width, not the dragged one ({} vs {settled})",
            c.get_panel_visible_w()
        );
    }
}
