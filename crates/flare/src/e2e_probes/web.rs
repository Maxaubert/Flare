//! The web panel probes (Surfaces stage 3), answered only under `--e2e` like every probe.
//!
//! `web.state` says where the browser window is, from three sides: what the browser itself holds
//! (the controller's bounds and the child windows of Flare's window), what Slint drew (the web box)
//! and what `flare_surfaces::web::web_rect` computes for the same page, so a scenario can hold the
//! three to each other at any display scaling. A script is started by the `WebScript` request (the
//! test's only way to look inside a page) and `web.script.result` reads the answer of the latest.

use serde_json::{Value, json};

use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("web.state", web_state);
    r.add("web.script.result", web_script_result);
}

fn scale_of(ctx: &ProbeCtx) -> f32 {
    let k = ctx.app.get_scale();
    if k.is_finite() && k > 0.0 { k } else { 1.0 }
}

fn rect_json(r: flare_surfaces::web::PhysRect) -> Value {
    json!({"x": r.x, "y": r.y, "w": r.w, "h": r.h, "right": r.right(), "bottom": r.bottom()})
}

fn web_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let a = ctx.app;
    let k = scale_of(ctx);
    let drawn = flare_surfaces::web::phys_rect(
        a.get_panel_web_x(),
        a.get_panel_web_y(),
        a.get_panel_web_w(),
        a.get_panel_web_h(),
        k,
    );
    let round = |v: f32| (v * k).round() as i32;
    let share = a.get_panel_share();
    let maths = flare_surfaces::web::web_rect(
        round(a.get_page_left()),
        round(a.get_panel_slot_top()),
        round(a.get_page_w()),
        round(a.get_panel_slot_h()),
        f64::from(k),
        (share > 0.0).then_some(share),
        round(a.get_panel_header_h() + a.get_panel_bar_h()),
    );
    let c = ctx
        .ctl
        .try_borrow()
        .map_err(|_| "web.state: the controller is busy".to_string())?;
    let mut v = c.web_probe();
    v["kind"] = json!(a.get_panel_kind());
    v["title"] = json!(a.get_panel_title().to_string());
    v["sub"] = json!(a.get_panel_sub().to_string());
    v["note"] = json!(a.get_panel_note().to_string());
    v["shown"] = json!(a.get_panel_shown());
    v["overlay_open"] = json!(a.get_overlay_open());
    v["scale"] = json!(k);
    v["bar"] = json!({
        "url": a.get_panel_web_url().to_string(),
        "draft": a.get_panel_web_draft().to_string(),
        "loading": a.get_panel_web_loading(),
        "can_back": a.get_panel_web_can_back(),
        "can_forward": a.get_panel_web_can_forward(),
        "missing_runtime": a.get_panel_web_missing_runtime(),
        "input_focused": a.get_panel_web_input_focused(),
        "h": a.get_panel_bar_h(),
    });
    v["ribbon"] = json!({
        "shown": a.get_panel_web_ribbon_shown(),
        "text": a.get_panel_web_ribbon().to_string(),
        "w": a.get_panel_web_ribbon_w(),
    });
    v["still"] = json!({
        "shown": a.get_panel_web_has_still(),
        "w": a.get_panel_web_still().size().width,
        "h": a.get_panel_web_still().size().height,
    });
    v["drawn"] = rect_json(drawn);
    v["maths"] = maths.map_or(Value::Null, rect_json);
    Ok(v)
}

fn web_script_result(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let c = ctx
        .ctl
        .try_borrow()
        .map_err(|_| "web.script.result: the controller is busy".to_string())?;
    Ok(match c.web_script_latest() {
        None => json!({"done": false}),
        Some(Ok(json_text)) => json!({"done": true, "ok": true, "value": json_text}),
        Some(Err(e)) => json!({"done": true, "ok": false, "error": e}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rect_is_written_with_its_far_edges() {
        let v = rect_json(flare_surfaces::web::PhysRect {
            x: 10,
            y: 20,
            w: 30,
            h: 40,
        });
        assert_eq!(v["right"], json!(40));
        assert_eq!(v["bottom"], json!(60));
    }
}
