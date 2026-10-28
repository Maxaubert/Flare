//! The probe of stage 8 (Surfaces): what the window and the hub hold about mods. Read-only, like every probe,
//! and it never carries a token: the scenario sees which mods are on, which tools the agent has, which pages are
//! bound to which tabs and how many calls wait.

use serde_json::{Value, json};

use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("mods.state", state);
    r.add("plugin.ask", plugin_ask);
}

/// `plugin.ask` (the `fill` permission): whether a plugin's question is up, its kind, plugin and host, what the
/// dialog says, where its buttons are (css px, for the scenario's pointer: it presses them as the person would)
/// and whether the secret field has the keys. Never a value: the dialog holds none, and the field's text is not read.
fn plugin_ask(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let c = ctx
        .ctl
        .try_borrow()
        .map_err(|_| "plugin.ask: the controller is busy".to_string())?;
    let a = ctx.app;
    let rect = |x: f32, y: f32, w: f32, h: f32| json!({"x": x, "y": y, "w": w, "h": h});
    let mut v = c.e2e_plugin_ask();
    v["open"] = json!(a.get_pask_open());
    v["title"] = json!(a.get_pask_title().to_string());
    v["body"] = json!(format!("{:?}", a.get_pask_body()));
    v["label"] = json!(a.get_pask_label().to_string());
    v["field_focus"] = json!(a.get_pask_field_focus());
    v["overlay_open"] = json!(a.get_overlay_open());
    v["buttons"] = json!({
        "allow": rect(a.get_pask_allow_x(), a.get_pask_allow_y(), a.get_pask_allow_w(), a.get_pask_allow_h()),
        "deny": rect(a.get_pask_deny_x(), a.get_pask_deny_y(), a.get_pask_deny_w(), a.get_pask_deny_h()),
        "session": rect(a.get_pask_session_x(), a.get_pask_session_y(), a.get_pask_session_w(), a.get_pask_session_h()),
        "ok": rect(a.get_pask_ok_x(), a.get_pask_ok_y(), a.get_pask_ok_w(), a.get_pask_ok_h()),
    });
    Ok(v)
}

/// `mods.state`: `null` when this run has no mods, else the hub's snapshot, the tools, the bound pages and the
/// pending calls, with the window's panel kind.
fn state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let c = ctx
        .ctl
        .try_borrow()
        .map_err(|_| "mods.state: the controller is busy".to_string())?;
    let mut v = c.e2e_mods_state();
    if v.is_object() {
        v["panel_kind"] = json!(ctx.app.get_panel_kind());
        v["panel_title"] = json!(ctx.app.get_panel_title().to_string());
    }
    Ok(v)
}
