//! The vertical tabs probes (Task 23): `sidebar.state` answers where the sidebar, its rows, its
//! find box, its grip and its New tab button are, and `corner.state` answers the numbers of the cut
//! corner where the bar meets the sidebar. Both are read-only.
//!
//! The window exports the sidebar's mode, width and rows but not the rectangles inside it, so the
//! geometry below is the same arithmetic `sidebar.slint` uses (a 50 px find block, 46 px cards
//! 4 px apart, 8 px in from each side). The scenario checks it against painted pixels, so a layout
//! that drifts from these numbers fails there, not here.

use serde_json::{Value, json};
use slint::{ComponentHandle, Model};

use super::{ProbeCtx, ProbeRegistry};
use crate::Theme;

/// The title bar's height; the sidebar starts under it.
pub const BAR_H: f32 = 36.0;
/// The find box block above the rows.
pub const FIND_H: f32 = 50.0;
/// One card, and the gap under it.
pub const ROW_H: f32 = 46.0;
pub const ROW_GAP: f32 = 4.0;
/// A card is this far in from each side of the sidebar.
pub const CARD_INSET: f32 = 8.0;
/// The New tab block at the foot: a hairline, 8, the 34 px button, 8.
pub const FOOT_BUTTON_H: f32 = 34.0;
pub const FOOT_PAD: f32 = 8.0;
/// The corner radius where the terminal meets the bar and the sidebar.
pub const CORNER_R: f32 = 10.0;
/// The resize grip: 6 px wide, centred on the sidebar's right edge.
pub const GRIP_W: f32 = 6.0;
/// The path both halves of the corner draw: the fill outside the arc, the arc itself.
pub const CORNER_FILL: &str = "M0 0H10A10 10 0 0 0 0 10Z";
pub const CORNER_ARC: &str = "M10 0A10 10 0 0 0 0 10";

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("sidebar.state", sidebar_state);
    r.add("corner.state", corner_state);
}

/// A css rectangle with its device-pixel twin.
fn rect_json(x: f32, y: f32, w: f32, h: f32, k: f32) -> Value {
    let d = |v: f32| (v * k).round() as i64;
    json!({
        "x": x, "y": y, "w": w, "h": h, "right": x + w, "bottom": y + h,
        "dx": d(x), "dy": d(y), "dw": d(w), "dh": d(h),
    })
}

/// The top of each row's slot in css px from the window's top. A row the find box hides keeps its
/// 4 px of spacing and loses its height, as the layout leaves it.
pub fn row_tops(kept: &[bool]) -> Vec<f32> {
    let mut y = BAR_H + FIND_H;
    let mut out = Vec::with_capacity(kept.len());
    for &k in kept {
        out.push(y);
        y += if k { ROW_H } else { 0.0 } + ROW_GAP;
    }
    out
}

/// A card's rectangle (x, y, w, h) for a slot top and the sidebar's width.
pub fn card_rect(top: f32, side_w: f32) -> (f32, f32, f32, f32) {
    (CARD_INSET, top, side_w - 2.0 * CARD_INSET, ROW_H)
}

/// The icon band: a square the card's height at the card's left edge.
pub fn band_rect(top: f32) -> (f32, f32, f32, f32) {
    (CARD_INSET, top, ROW_H, ROW_H)
}

/// Where the sidebar's right edge is, in css px: its width when it is pinned or slid out, 10 px
/// left of the window's edge (the hidden panel waits `side_w + CORNER_R` off the edge) otherwise.
pub fn nav_right(side_w: f32, open: bool, revealed: bool) -> f32 {
    if open || revealed { side_w } else { -CORNER_R }
}

/// The New tab button's rectangle: across the foot, 8 in from each side, 8 above the bottom.
pub fn new_tab_rect(side_w: f32, window_h: f32) -> (f32, f32, f32, f32) {
    (
        FOOT_PAD,
        window_h - FOOT_PAD - FOOT_BUTTON_H,
        side_w - 2.0 * FOOT_PAD,
        FOOT_BUTTON_H,
    )
}

/// `#rrggbb` of a colour, `#rrggbbaa` when it is not opaque.
fn hex(c: slint::Color) -> String {
    if c.alpha() == 255 {
        format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue())
    } else {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            c.red(),
            c.green(),
            c.blue(),
            c.alpha()
        )
    }
}

fn mark_motion(kind: i32) -> &'static str {
    match kind {
        1 => "ring",
        2 => "line",
        3 => "run",
        _ => "none",
    }
}

fn sidebar_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let a = ctx.app;
    let k = a.window().scale_factor();
    let size = a.window().size();
    let (win_w, win_h) = (size.width as f32 / k, size.height as f32 / k);
    let vertical = a.get_layout_vertical();
    let collapsed = a.get_sidebar_collapsed();
    let revealed = a.get_sidebar_revealed();
    let w = a.get_sidebar_width();
    let open = vertical && !collapsed;
    let shown = vertical && (open || revealed);
    let right = nav_right(w, open, revealed);
    let rows: Vec<crate::TabRow> = a.get_tab_rows().iter().collect();
    let kept: Vec<bool> = rows.iter().map(|r| r.kept).collect();
    let tops = row_tops(&kept);
    let row_list: Vec<Value> = rows
        .iter()
        .zip(&tops)
        .map(|(r, &top)| {
            let (cx, cy, cw, ch) = card_rect(top, w);
            let (bx, by, bw, bh) = band_rect(top);
            json!({
                "id": r.id.to_string(),
                "label": r.label.to_string(),
                "folder": r.folder.to_string(),
                "front": r.active,
                "kept": r.kept,
                "settings": r.settings,
                "tinted": r.active,
                "mark": {
                    "state": r.mark_state,
                    "motion": mark_motion(r.mark_kind),
                },
                "rect": rect_json(cx, cy + r.offset, cw, ch, k),
                "icon_band": {
                    "x": bx, "y": by, "w": bw, "h": bh,
                    "dx": (bx * k).round() as i64, "dy": (by * k).round() as i64,
                    "tinted": r.active,
                },
            })
        })
        .collect();
    let seen = rows.iter().filter(|r| r.kept).count();
    let overlay = a.get_overflow_open() || a.get_dialog_open() || a.get_update_open();
    let t = a.global::<Theme>();
    Ok(json!({
        "vertical": vertical,
        "strip": !vertical && !rows.is_empty(),
        "open": open,
        "collapsed": vertical && collapsed,
        "shown": shown,
        "sliding": a.get_sidebar_sliding(),
        // Where the slide has the panel and the slot now (the values drawn), and how many OS frames came:
        // a slid-out picture with no sidebar names its cause (rule 345).
        "slide_now": a.get_sidebar_slide_now(),
        "slot_now": a.get_sidebar_slot_now(),
        "redraws": crate::e2e_glue::redraw_count(),
        "mode": if collapsed { "collapsed" } else { "open" },
        "w": w,
        "nav": rect_json(right - w, BAR_H, w, win_h - BAR_H, k),
        "host_left": if open { w } else { 0.0 },
        "window": { "w": win_w, "h": win_h },
        "find": {
            "value": a.get_sidebar_find().to_string(),
            "focus": a.get_chrome_has_focus() && !overlay,
        },
        "seen": seen,
        "rows": row_list,
        "grip": if shown {
            let (gx, gy) = (right - GRIP_W / 2.0, BAR_H);
            rect_json(gx, gy, GRIP_W, win_h - BAR_H, k)
        } else {
            Value::Null
        },
        "new_tab": if shown {
            let (x, y, nw, nh) = new_tab_rect(w, win_h);
            rect_json(x + (right - w), y, nw, nh, k)
        } else {
            Value::Null
        },
        "bar": rect_json(0.0, 0.0, win_w, BAR_H, k),
        "theme": {
            "secondary": hex(t.get_secondary()),
            "ground": hex(t.get_ground()),
            "divider": hex(t.get_divider()),
            "icon": hex(t.get_icon()),
        },
    }))
}

fn corner_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let a = ctx.app;
    let k = a.window().scale_factor();
    let vertical = a.get_layout_vertical();
    let open = vertical && !a.get_sidebar_collapsed();
    let revealed = vertical && a.get_sidebar_revealed();
    let w = a.get_sidebar_width();
    let right = nav_right(w, open, revealed);
    // The hairline is max(1, floor(scale)) device pixels.
    let hair = k.floor().max(1.0) / k;
    Ok(json!({
        "vertical": vertical,
        "cut": open,
        "slid": !open && revealed,
        "r": CORNER_R,
        "x": right,
        "y": BAR_H,
        "dx": (right * k).round() as i64,
        "dy": (BAR_H * k).round() as i64,
        "hair": hair,
        "fill_path": CORNER_FILL,
        "arc_path": CORNER_ARC,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_stand_one_under_another_from_under_the_find_box() {
        let tops = row_tops(&[true, true, true]);
        assert_eq!(tops, vec![86.0, 136.0, 186.0]);
    }

    #[test]
    fn a_row_the_find_box_hides_keeps_its_gap_and_loses_its_height() {
        let tops = row_tops(&[true, false, true]);
        assert_eq!(tops, vec![86.0, 136.0, 140.0]);
    }

    #[test]
    fn a_card_is_inset_eight_px_each_side() {
        assert_eq!(card_rect(86.0, 220.0), (8.0, 86.0, 204.0, 46.0));
    }

    #[test]
    fn the_icon_band_is_a_square_the_cards_height_at_its_left() {
        let (x, y, w, h) = band_rect(86.0);
        assert_eq!((x, y), (8.0, 86.0));
        assert_eq!(w, h);
        assert_eq!(h, ROW_H);
    }

    #[test]
    fn the_hidden_panel_waits_ten_px_off_the_edge() {
        assert_eq!(nav_right(220.0, true, false), 220.0);
        assert_eq!(nav_right(220.0, false, true), 220.0);
        assert_eq!(nav_right(220.0, false, false), -10.0);
    }

    #[test]
    fn the_new_tab_button_runs_across_the_foot() {
        assert_eq!(new_tab_rect(220.0, 700.0), (8.0, 658.0, 204.0, 34.0));
    }

    #[test]
    fn the_motion_names_are_the_strips() {
        assert_eq!(mark_motion(1), "ring");
        assert_eq!(mark_motion(2), "line");
        assert_eq!(mark_motion(3), "run");
        assert_eq!(mark_motion(0), "none");
    }

    #[test]
    fn a_rect_carries_its_right_edge_and_its_device_twin() {
        let r = rect_json(8.0, 86.0, 204.0, 46.0, 2.25);
        assert_eq!(r["right"], json!(212.0));
        assert_eq!(r["dx"], json!(18));
        assert_eq!(r["dw"], json!(459));
    }

    #[test]
    fn the_corner_paths_are_one_circle() {
        assert!(CORNER_FILL.contains("A10 10"));
        assert!(CORNER_ARC.contains("A10 10"));
        assert_eq!(CORNER_R, 10.0);
    }
}
