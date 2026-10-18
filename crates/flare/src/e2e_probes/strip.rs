//! The tab strip probes (Task 22): strip.state answers how the strip holds its tabs (drawn, as
//! icons, hidden behind +N, listed, and whether the list has the keys), and tip.state answers the
//! tooltip the window shows. Both are read-only; the pure answers below are what the unit tests pin.

use serde_json::{Value, json};
use slint::Model;

use super::{ProbeCtx, ProbeRegistry};

/// What the strip holds, counted from its rows.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Tabs drawn in the bar (named or as icons).
    pub drawn: usize,
    /// Drawn tabs that are icons only (mode 1).
    pub icons: usize,
    /// Tabs hidden behind +N (mode 2).
    pub hidden: usize,
    /// How far the drawn tabs run past the strip's right edge, in device px. Flare's strip has no
    /// scroll offset, so 0 means nothing is pushed out of the bar.
    pub spill_px: i64,
    /// The tab in front is drawn with its name (mode 0 and active).
    pub active_named: bool,
}

/// Counts the rows (mode, width_px, active), where mode 0 is named, 1 icon and 2 hidden, against
/// the strip's width in device px.
pub fn summarise(rows: &[(i64, i64, bool)], strip_px: i64) -> Summary {
    let mut s = Summary::default();
    let mut used: i64 = 0;
    for &(mode, w, active) in rows {
        if mode == 2 {
            s.hidden += 1;
            continue;
        }
        s.drawn += 1;
        used += w;
        if mode == 1 {
            s.icons += 1;
        }
        if mode == 0 && active {
            s.active_named = true;
        }
    }
    s.spill_px = (used - strip_px).max(0);
    s
}

/// The strip.state answer. listed is the list's row count (0 while it is shut), and
/// find_focus stands for the list's field having the keys, which the window only grants while
/// the list is open.
pub fn strip_answer(s: &Summary, list_open: bool, list_rows: usize) -> Value {
    json!({
        "scroll": s.spill_px,
        "spill_px": s.spill_px,
        "drawn": s.drawn,
        "icons": s.icons,
        "overflow_n": s.hidden,
        "active_named": s.active_named,
        "list_open": list_open,
        "list_rows": list_rows,
        "listed": list_rows,
        "find_focus": list_open,
    })
}

/// The tip.state answer: the text, whether it shows, and where (css px). A hidden tip has no
/// rectangle.
pub fn tip_answer(visible: bool, text: &str, rect: (f32, f32, f32, f32)) -> Value {
    let (x, y, w, h) = rect;
    json!({
        "text": text,
        "visible": visible,
        "rect": if visible {
            json!({ "x": x, "y": y, "w": w, "h": h })
        } else {
            Value::Null
        },
    })
}

/// Adds this file's probes to r.
pub fn register(r: &mut ProbeRegistry) {
    r.add("strip.state", strip_state);
    r.add("tip.state", tip_state);
}

fn strip_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let rows: Vec<(i64, i64, bool)> = ctx
        .app
        .get_tab_rows()
        .iter()
        .map(|r| (r.mode as i64, r.w as i64, r.active))
        .collect();
    let summary = summarise(&rows, ctx.app.get_strip_phys_width() as i64);
    let open = ctx.app.get_overflow_open();
    let list_rows = ctx.app.get_overflow_rows().row_count();
    let mut answer = strip_answer(&summary, open, if open { list_rows } else { 0 });
    // The +N button sits at the strip's geometry (device px from the strip's left edge), and the
    // first listed row is the one Enter picks.
    let geom = ctx.app.get_strip_geom();
    let first = ctx
        .app
        .get_overflow_rows()
        .iter()
        .next()
        .map(|r| r.label.to_string())
        .unwrap_or_default();
    // The strip's own width in device px: the room the tabs share (tabLooks works out the fit).
    answer["strip_px"] = json!(ctx.app.get_strip_phys_width() as i64);
    answer["more"] = json!({ "x_px": geom.more_x as i64, "w_px": geom.more_w as i64 });
    answer["list_first"] = json!(first);
    Ok(answer)
}

fn tip_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let a = ctx.app;
    Ok(tip_answer(
        a.get_tip_visible(),
        a.get_tip_text().as_ref(),
        (a.get_tip_x(), a.get_tip_y(), a.get_tip_w(), a.get_tip_h()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spill_is_how_far_the_drawn_tabs_run_past_the_strip() {
        // Two named tabs of 150 device px in a 200 px strip: 300 - 200 = 100 px pushed out.
        let rows = [(0, 150, true), (0, 150, false)];
        assert_eq!(summarise(&rows, 200).spill_px, 100);
    }

    #[test]
    fn nothing_spills_when_the_drawn_tabs_fit() {
        let rows = [(0, 150, false), (1, 60, true)];
        let s = summarise(&rows, 400);
        assert_eq!(s.spill_px, 0);
        assert_eq!(s.drawn, 2);
        assert_eq!(s.icons, 1);
    }

    #[test]
    fn hidden_tabs_are_counted_apart_from_drawn_ones() {
        let rows = [(0, 150, true), (1, 60, false), (2, 0, false), (2, 0, false)];
        let s = summarise(&rows, 1000);
        assert_eq!(s.drawn, 2);
        assert_eq!(s.hidden, 2);
        assert!(s.active_named);
    }

    #[test]
    fn the_front_tab_counts_as_named_only_when_it_is_not_an_icon() {
        let rows = [(1, 60, true)];
        assert!(!summarise(&rows, 1000).active_named);
    }

    #[test]
    fn the_strip_answer_says_the_list_has_the_keys_only_while_it_is_open() {
        let s = Summary {
            hidden: 3,
            ..Summary::default()
        };
        let shut = strip_answer(&s, false, 0);
        assert_eq!(shut["find_focus"], json!(false));
        assert_eq!(shut["list_rows"], json!(0));
        assert_eq!(shut["overflow_n"], json!(3));
        let open = strip_answer(&s, true, 3);
        assert_eq!(open["find_focus"], json!(true));
        assert_eq!(open["listed"], json!(3));
    }

    #[test]
    fn a_hidden_tip_has_no_rectangle_and_a_shown_one_has_its_box() {
        let hidden = tip_answer(false, "C:\\a", (1.0, 2.0, 3.0, 4.0));
        assert_eq!(hidden["visible"], json!(false));
        assert!(hidden["rect"].is_null());
        let shown = tip_answer(true, "C:\\a", (1.0, 2.0, 3.0, 4.0));
        assert_eq!(shown["rect"]["w"], json!(3.0));
        assert_eq!(shown["text"], json!("C:\\a"));
    }
}
