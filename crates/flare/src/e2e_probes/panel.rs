//! The panel probes (Surfaces stage 1): `panel.state` answers what the panel slot shows and where
//! everything in it is, in css px with the device-pixel twin the other geometry probes give, and
//! next to it the numbers `flare_surfaces::layout` computes for the same page, so a scenario can
//! hold the drawn slot against the maths at any display scaling. Read-only, like every probe.

use serde_json::{Value, json};

use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("panel.state", panel_state);
    r.add("convo.state", convo_state);
    r.add("ctl.info", ctl_info);
}

/// The control channel of this run: its pipe, the MCP server beside the exe, and every shell tab's id
/// and secret. The scenario plays the part of a shell (it presents the token a real shell would be
/// handed through its environment). Only ever answered under `--e2e`, like every probe.
fn ctl_info(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let c = ctx
        .ctl
        .try_borrow()
        .map_err(|_| "ctl.info: the controller is busy".to_string())?;
    Ok(c.ctl_probe())
}

/// A css rectangle with its device-pixel twin.
pub fn rect_json(x: f32, y: f32, w: f32, h: f32, k: f32) -> Value {
    let d = |v: f32| (v * k).round() as i64;
    json!({
        "x": x, "y": y, "w": w, "h": h, "right": x + w, "bottom": y + h,
        "dx": d(x), "dy": d(y), "dw": d(w), "dh": d(h),
    })
}

fn scale_of(ctx: &ProbeCtx) -> f32 {
    let k = ctx.app.get_scale();
    if k.is_finite() && k > 0.0 { k } else { 1.0 }
}

/// The slot, the splitter, the body, the picture and the close button, in the window's css px.
/// Errors when the slot is not on the screen.
pub fn rects(ctx: &ProbeCtx, name: &str) -> Result<Value, String> {
    let a = ctx.app;
    let k = scale_of(ctx);
    if !a.get_panel_shown() {
        return Err(format!("ui.rect {name}: no panel is showing"));
    }
    let left = a.get_grid_left();
    let top = a.get_grid_top();
    let slot_x = left + a.get_panel_slot_x();
    let slot_w = a.get_panel_visible_w();
    match name {
        "panel" => Ok(rect_json(slot_x, top, slot_w, a.get_panel_slot_h(), k)),
        "splitter" => Ok(rect_json(
            a.get_panel_splitter_x(),
            top,
            a.get_panel_splitter_w(),
            a.get_panel_slot_h(),
            k,
        )),
        "panel-body" => Ok(rect_json(
            a.get_panel_body_x(),
            a.get_panel_body_y(),
            a.get_panel_body_w(),
            a.get_panel_body_h(),
            k,
        )),
        // A row of the Changes file list (the list at its top): the whole width of the body.
        n if n.starts_with("convo-file-") && a.get_panel_kind() == 4 => {
            let i: f32 = n["convo-file-".len()..]
                .parse()
                .map_err(|_| format!("ui.rect {n}: not a row number"))?;
            let h = a.get_panel_convo_list_row_h();
            Ok(rect_json(
                a.get_panel_body_x(),
                a.get_panel_body_y() + i * h,
                a.get_panel_body_w(),
                h,
                k,
            ))
        }
        "panel-image" if a.get_panel_kind() == 1 => Ok(rect_json(
            a.get_panel_image_x(),
            a.get_panel_image_y(),
            a.get_panel_image_w(),
            a.get_panel_image_h(),
            k,
        )),
        "panel-image" => Err("ui.rect panel-image: the panel shows no picture".into()),
        // The close button: 20 px, 8 px in from the slot's right edge, centred in the 32 px header.
        "panel-close" => {
            let head = a.get_panel_header_h();
            Ok(rect_json(
                slot_x + slot_w - 8.0 - 20.0,
                top + (head - 20.0) / 2.0,
                20.0,
                20.0,
                k,
            ))
        }
        // The header's wrap button (a text file).
        "panel-wrap" if a.get_panel_kind() == 2 => Ok(rect_json(
            a.get_panel_wrap_button_x(),
            a.get_panel_wrap_button_y(),
            a.get_panel_wrap_button_w(),
            a.get_panel_wrap_button_w(),
            k,
        )),
        "panel-wrap" => Err("ui.rect panel-wrap: the panel shows no text".into()),
        _ => Err(format!("ui.rect: no such rectangle: {name}")),
    }
}

/// What the conversation panels show and what runs for them (Surfaces stage 6): the kind, the rows, the picks, how
/// many rows the window built (the virtualised views) and how many transcript trackers exist.
fn convo_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    use slint::Model;
    let a = ctx.app;
    let files: Vec<Value> = a
        .get_panel_convo_files()
        .iter()
        .map(|f| json!({"name": f.name.to_string(), "meta": f.meta.to_string(), "path": f.path.to_string()}))
        .collect();
    let rows = a.get_panel_convo_rows();
    let diff: Vec<Value> = rows
        .iter()
        .take(60)
        .map(|r| json!({"kind": r.kind, "old": r.old_no.to_string(), "new": r.new_no.to_string(), "text": r.text.to_string()}))
        .collect();
    let tasks: Vec<Value> = a
        .get_panel_convo_tasks()
        .iter()
        .map(|t| json!({"kind": t.kind, "state": t.state, "text": t.text.to_string(), "sub": t.sub.to_string(), "active": t.active, "collapsed": t.collapsed}))
        .collect();
    let (trackers, sessions) = ctx.ctl.try_borrow().map_or((0, Vec::new()), |c| {
        let s: Vec<String> = c.convo().sessions();
        (c.convo().tracker_count(), s)
    });
    // The Claude session the hooks named for the tab in front (the scenario waits for it).
    let front_session = ctx.ctl.try_borrow().ok().and_then(|c| {
        let tab = c.front_id()?;
        c.hub().claude_session(&tab).map(str::to_string)
    });
    Ok(json!({
        "front_session": front_session,
        "kind": a.get_panel_kind(),
        "shown": a.get_panel_shown(),
        "title": a.get_panel_title().to_string(),
        "sub": a.get_panel_sub().to_string(),
        "note": a.get_panel_note().to_string(),
        "selected": a.get_panel_convo_selected().to_string(),
        "files": files,
        "diff_rows": rows.row_count(),
        "diff": diff,
        "tasks": tasks,
        "list_built": a.get_panel_convo_list_built(),
        "diff_built": a.get_panel_convo_diff_built(),
        "trackers": trackers,
        "sessions": sessions,
    }))
}

fn panel_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let a = ctx.app;
    let k = scale_of(ctx);
    let page_phys = (a.get_page_w() * k).round() as i32;
    let share = a.get_panel_share();
    let want = flare_surfaces::layout::slot_layout(
        page_phys,
        f64::from(k),
        (share > 0.0).then_some(share),
    );
    let pic = a.get_panel_picture().size();
    Ok(json!({
        "open": a.get_panel_open(),
        "shown": a.get_panel_shown(),
        "fits": a.get_panel_fits(),
        "kind": a.get_panel_kind(),
        "title": a.get_panel_title().to_string(),
        "sub": a.get_panel_sub().to_string(),
        "note": a.get_panel_note().to_string(),
        "foot": a.get_panel_foot().to_string(),
        "share": share,
        "has_focus": a.get_panel_has_focus(),
        "overlay_open": a.get_overlay_open(),
        "zoomed": a.get_panel_image_zoomed(),
        "can_zoom": a.get_panel_image_can_zoom(),
        "picture": { "w": pic.width, "h": pic.height },
        "alpha": a.get_panel_alpha(),
        "lines": slint::Model::row_count(&a.get_panel_lines()),
        "row0": slint::Model::row_data(&a.get_panel_lines(), 0).map(|r| r.to_string()),
        "first_line": a.get_panel_first_line(),
        "gutter_w": a.get_panel_gutter_w(),
        // Wrapping (2026-10-22): the columns that fit, whether the rows are wrapped, the setting, and how many wraps ran.
        "wrap_cols": a.get_panel_wrap_cols(),
        "wrapped": a.get_panel_wrapped(),
        "wrapped_cols": a.get_panel_wrapped_cols(),
        "wrap_on": a.get_panel_wrap_on(),
        "grid_fits": ctx.ctl.try_borrow().map_or(0, |c| c.grid_fit_count()),
        "wrap_runs": ctx.ctl.try_borrow().map_or(0, |c| c.panel_wrap_count()),
        "splitting": a.get_panel_splitting(),
        "visible_w": a.get_panel_visible_w(),
        "drag_w": a.get_panel_drag_w(),
        "page_phys_w": page_phys,
        "slot_phys_w": (a.get_panel_slot_w() * k).round() as i32,
        "grid_phys_w": a.get_grid_phys_width(),
        // What the layout maths says for this page: the scenario holds the drawn slot to it.
        "layout": {
            "slot_w": want.slot_w, "grid_w": want.grid_w, "shown": want.shown,
            "min_w": want.min_w, "max_w": want.max_w,
        },
        "ctl": {
            "open_panels": ctx.ctl.try_borrow().map_or(0, |c| c.open_panels()),
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rect_carries_its_device_pixels_at_every_scale() {
        for k in [1.0f32, 1.5, 2.25, 3.0] {
            let r = rect_json(10.5, 20.25, 100.0, 50.0, k);
            assert_eq!(r["dx"], json!((10.5 * k).round() as i64));
            assert_eq!(r["right"], json!(110.5));
            assert_eq!(r["bottom"], json!(70.25));
        }
    }
}
