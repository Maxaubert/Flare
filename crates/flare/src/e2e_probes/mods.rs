//! The probe of stage 8 (Surfaces): what the window and the hub hold about mods. Read-only, like every probe,
//! and it never carries a token: the scenario sees which mods are on, which tools the agent has, which pages are
//! bound to which tabs and how many calls wait.

use serde_json::{Value, json};

use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("mods.state", state);
    r.add("plugin.ask", plugin_ask);
    r.add("plugin.sheet", plugin_sheet);
}

/// `plugin.sheet`: whether a plugin's details sheet is open, for which plugin, its tiles (key, kind, on, the level
/// and its tone), where its box, grid, close X and foot buttons are (css px, for the scenario's pointer), which
/// dropdown is open and how far under its tile, and how far the sheet scrolls.
fn plugin_sheet(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    use crate::SettingsBridge;
    use slint::{ComponentHandle, Model};
    let a = ctx.app;
    let b = a.global::<SettingsBridge>();
    let tiles: Vec<Value> = b
        .get_sheet_tiles()
        .iter()
        .map(|t| {
            json!({"key": t.key.to_string(), "label": t.label.to_string(), "kind": t.kind, "on": t.on,
                "value": t.value.to_string(), "value_name": t.value_name.to_string(), "tone": t.tone,
                "detail": t.detail.to_string(), "tip": t.tip.to_string()})
        })
        .collect();
    Ok(json!({
        "open": b.get_sheet_open(),
        "name": b.get_sheet_name().to_string(),
        "sub": b.get_sheet_sub().to_string(),
        "branded": b.get_sheet_look().branded,
        "plate": b.get_sheet_look().plate,
        "tiles": tiles,
        "box": {"x": a.get_sheet_box_x(), "y": a.get_sheet_box_y(), "w": a.get_sheet_box_w(), "h": a.get_sheet_box_h()},
        "grid_y": a.get_sheet_grid_y(),
        "cell": {"w": a.get_sheet_cell_w(), "h": a.get_sheet_cell_h()},
        "close": {"x": a.get_sheet_close_x(), "y": a.get_sheet_close_y()},
        "menu_tile": a.get_sheet_menu_tile(),
        "menu_drop": a.get_sheet_menu_drop(),
        "scroll": {"y": a.get_sheet_scroll_y(), "max": a.get_sheet_scroll_max()},
        "can_turn_on": b.get_sheet_can_turn_on(),
        "turn_on": {"x": a.get_sheet_turn_on_x(), "y": a.get_sheet_turn_on_y(), "w": a.get_sheet_turn_on_w(), "h": a.get_sheet_turn_on_h()},
    }))
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
    v["sub"] = json!(a.get_pask_sub().to_string());
    v["label"] = json!(a.get_pask_label().to_string());
    v["button"] = json!(a.get_pask_button().to_string());
    v["site"] = json!(a.get_pask_site().to_string());
    v["asker"] = json!(a.get_pask_asker().to_string());
    v["foot"] = json!(a.get_pask_foot().to_string());
    v["branded"] = json!(a.get_pask_look().branded);
    v["plate"] = json!(a.get_pask_look().plate);
    v["revealed"] = json!(a.get_pask_revealed());
    v["box"] = rect(
        a.get_pask_box_x(),
        a.get_pask_box_y(),
        a.get_pask_box_w(),
        a.get_pask_box_h(),
    );
    v["field_focus"] = json!(a.get_pask_field_focus());
    v["allow_armed"] = json!(a.get_pask_allow_armed());
    v["overlay_open"] = json!(a.get_overlay_open());
    v["buttons"] = json!({
        "allow": rect(a.get_pask_allow_x(), a.get_pask_allow_y(), a.get_pask_allow_w(), a.get_pask_allow_h()),
        "deny": rect(a.get_pask_deny_x(), a.get_pask_deny_y(), a.get_pask_deny_w(), a.get_pask_deny_h()),
        "ok": rect(a.get_pask_ok_x(), a.get_pask_ok_y(), a.get_pask_ok_w(), a.get_pask_ok_h()),
        "eye": rect(a.get_pask_eye_x(), a.get_pask_eye_y(), 32.0, 42.0),
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
