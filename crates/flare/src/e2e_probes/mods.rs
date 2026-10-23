//! The probe of stage 8 (Surfaces): what the window and the hub hold about mods. Read-only, like every probe,
//! and it never carries a token: the scenario sees which mods are on, which tools the agent has, which pages are
//! bound to which tabs and how many calls wait.

use serde_json::{Value, json};

use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("mods.state", state);
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
