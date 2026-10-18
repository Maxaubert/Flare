//! Input A probes (Task 29): the front shell's mouse-reporting mode (`term.mouse`) and the
//! scrollbar the window draws beside the grid (`scrollbar.rect`). Read-only, like every probe.

use flare_term::click_caret::MouseTracking;
use serde_json::{Value, json};
use slint::ComponentHandle;

use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("term.mouse", term_mouse);
    r.add("scrollbar.rect", scrollbar_rect);
    r.add("ime.rect", ime_rect);
    r.add("pointer.shape", pointer_shape);
}

/// The name a mouse-reporting mode goes by in the answer.
fn tracking_name(m: MouseTracking) -> &'static str {
    match m {
        MouseTracking::None => "none",
        MouseTracking::X10 => "x10",
        MouseTracking::Vt200 => "vt200",
        MouseTracking::Drag => "drag",
        MouseTracking::Any => "any",
    }
}

/// What the mode says: `on` is true when a program has asked for the mouse at all.
fn mouse_answer(m: MouseTracking, sgr: bool) -> Value {
    json!({
        "tracking": tracking_name(m),
        "sgr": sgr,
        "on": !matches!(m, MouseTracking::None),
    })
}

/// The front shell's mouse mode (PT's "xterm took the mouse mode").
fn term_mouse(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let h = ctx
        .front
        .as_ref()
        .ok_or("term.mouse: no shell is in front")?;
    let mode = h.with_emu_quiet(|e| e.mode());
    Ok(mouse_answer(mode.mouse, mode.sgr_mouse))
}

/// What the scrollbar probe answers: no native gutter or steppers, and `slider` is the thumb's
/// `(x, y, w, h)` in css px (with its device-pixel twin) while the shell has history, else null.
fn scrollbar_answer(thumb: Option<(f32, f32, f32, f32)>, k: f32) -> Value {
    let d = |v: f32| (v * k).round() as i64;
    let slider = thumb.map(|(x, y, w, h)| {
        json!({
            "x": x, "y": y, "w": w, "h": h,
            "dx": d(x), "dy": d(y), "dw": d(w), "dh": d(h),
        })
    });
    json!({ "gutter": 0, "slider": slider, "native": false })
}

/// The scrollbar beside the grid: the app draws a 6 px strip over the grid's right edge.
fn scrollbar_rect(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let a = ctx.app;
    let thumb = a.get_scroll_on().then(|| {
        (
            a.get_scroll_thumb_x(),
            a.get_scroll_thumb_y(),
            a.get_scroll_thumb_w(),
            a.get_scroll_thumb_h(),
        )
    });
    Ok(scrollbar_answer(thumb, a.window().scale_factor()))
}
/// The input method's rectangle in window device pixels, as last handed to the window.
fn ime_rect(_ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let a = crate::ime::last_area().ok_or("ime.rect: no frame has placed the rectangle yet")?;
    Ok(json!({ "x": a.x, "y": a.y, "w": a.w, "h": a.h }))
}

/// The name of a mouse cursor from its debug text: text, pointer or default.
fn shape_name(debug: &str) -> &'static str {
    if debug.contains("Text") {
        "text"
    } else if debug.contains("Pointer") {
        "pointer"
    } else {
        "default"
    }
}

/// The pointer shape the grid shows now.
fn pointer_shape(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let shape = shape_name(&format!("{:?}", ctx.app.get_grid_cursor()));
    Ok(json!({ "shape": shape }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_debug_text_names_its_shape() {
        assert_eq!(shape_name("BuiltIn(Text)"), "text");
        assert_eq!(shape_name("BuiltIn(Pointer)"), "pointer");
        assert_eq!(shape_name("BuiltIn(Default)"), "default");
    }

    #[test]
    fn every_mouse_mode_has_its_name_and_only_none_is_off() {
        assert_eq!(tracking_name(MouseTracking::None), "none");
        assert_eq!(tracking_name(MouseTracking::X10), "x10");
        assert_eq!(tracking_name(MouseTracking::Vt200), "vt200");
        assert_eq!(tracking_name(MouseTracking::Drag), "drag");
        assert_eq!(tracking_name(MouseTracking::Any), "any");
        let on = mouse_answer(MouseTracking::Vt200, true);
        assert_eq!(on["on"], true);
        assert_eq!(on["sgr"], true);
        let off = mouse_answer(MouseTracking::None, false);
        assert_eq!(off["on"], false);
        assert_eq!(off["tracking"], "none");
    }

    #[test]
    fn a_shell_with_history_answers_a_six_px_slider_and_none_answers_null() {
        let a = scrollbar_answer(Some((394.0, 10.0, 6.0, 80.0)), 2.25);
        assert_eq!(a["slider"]["w"].as_f64(), Some(6.0));
        assert_eq!(a["slider"]["dw"], 14);
        assert_eq!(a["gutter"], 0);
        assert_eq!(a["native"], false);
        assert!(scrollbar_answer(None, 1.0)["slider"].is_null());
    }
    #[test]
    fn the_probes_are_registered_once() {
        let mut r = ProbeRegistry::new();
        register(&mut r);
        let names = r.names();
        assert!(names.contains(&"term.mouse"));
        assert!(names.contains(&"scrollbar.rect"));
        assert!(names.contains(&"ime.rect"));
        assert!(names.contains(&"pointer.shape"));
    }
}
