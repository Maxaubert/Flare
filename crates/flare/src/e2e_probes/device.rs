//! The device panel's probe (issue #248, PR 4a), answered only under `--e2e` like every probe.
//!
//! `device.state` says what the front device panel shows (its setup state, its device, attached or paused, who
//! drives it), how many frames were handed to Slint (the e2e reads frames here, not from pixels: a Mac e2e window
//! may get no frames while it counts as occluded, rule 1117), the person's inputs and takeovers, every input the
//! fake source was given (in device points), and where the window drew the box, the picture and the setup button.

use serde_json::{Value, json};

use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("device.state", device_state);
}

fn device_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let a = ctx.app;
    let c = ctx
        .ctl
        .try_borrow()
        .map_err(|_| "device.state: the controller is busy".to_string())?;
    let mut v = c.device_probe();
    let rect = |x: f32, y: f32, w: f32, h: f32| json!({"x": x, "y": y, "w": w, "h": h});
    v["kind"] = json!(a.get_panel_kind());
    v["shown"] = json!(a.get_panel_shown());
    v["title"] = json!(a.get_panel_title().to_string());
    v["line"] = json!(a.get_panel_device_line().to_string());
    v["button"] = json!(a.get_panel_device_button().to_string());
    v["button_shown"] = json!(a.get_panel_device_button_shown());
    v["slow"] = json!(a.get_panel_device_slow().to_string());
    v["still"] = json!(a.get_panel_device_still().to_string());
    v["foot"] = json!(a.get_panel_device_foot().to_string());
    v["controls"] = json!(a.get_panel_device_controls_shown());
    v["has_frame"] = json!(a.get_panel_device_has_frame());
    v["box"] = rect(
        a.get_panel_device_x(),
        a.get_panel_device_y(),
        a.get_panel_device_w(),
        a.get_panel_device_h(),
    );
    v["picture"] = rect(
        a.get_panel_device_pic_x(),
        a.get_panel_device_pic_y(),
        a.get_panel_device_pic_w(),
        a.get_panel_device_pic_h(),
    );
    v["button_rect"] = rect(
        a.get_panel_device_button_x(),
        a.get_panel_device_button_y(),
        a.get_panel_device_button_w(),
        a.get_panel_device_button_h(),
    );
    Ok(v)
}
