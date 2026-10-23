//! The probes of stage 7 (Surfaces): the Preview chip and the question notifications.
//! Read-only, like every probe. The notification recorder is the seam that stands in for Windows' toast under
//! `--e2e`: nothing reaches the desktop, and the scenario reads what would have been shown.

use std::sync::Mutex;

use serde_json::{Value, json};

use super::{ProbeCtx, ProbeRegistry};
use crate::e2e_probes::panel::rect_json;

/// A notification the app asked for.
#[derive(Clone, Debug)]
struct Recorded {
    tab: String,
    title: String,
    body: String,
}

static TOASTS: Mutex<Vec<Recorded>> = Mutex::new(Vec::new());

/// Records one notification that would have been shown. The window's sink calls it under `--e2e` only.
pub fn record_toast(tab: &str, title: &str, body: &str) {
    TOASTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Recorded {
            tab: tab.to_string(),
            title: title.to_string(),
            body: body.to_string(),
        });
}

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("surf7.state", state);
    r.add("notify.log", notify_log);
}

/// The chip and the counters of stage 7.
fn state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let c = ctx
        .ctl
        .try_borrow()
        .map_err(|_| "surf7.state: the controller is busy".to_string())?;
    let mut v = c.surf7_probe();
    let a = ctx.app;
    v["panel_kind"] = json!(a.get_panel_kind());
    Ok(v)
}

/// What the notification seam recorded.
fn notify_log(_ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let rows: Vec<Value> = TOASTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|r| json!({"tab": r.tab, "title": r.title, "body": r.body}))
        .collect();
    Ok(json!({ "toasts": rows }))
}

/// The chip, for `ui.rect`.
pub fn rects(ctx: &ProbeCtx, name: &str) -> Result<Value, String> {
    let a = ctx.app;
    let k = {
        let k = a.get_scale();
        if k.is_finite() && k > 0.0 { k } else { 1.0 }
    };
    match name {
        "preview-chip" => {
            if a.get_preview_chip_label().is_empty() {
                return Err("ui.rect preview-chip: no chip is showing".into());
            }
            Ok(rect_json(
                a.get_preview_chip_x(),
                a.get_preview_chip_y(),
                a.get_preview_chip_w(),
                a.get_preview_chip_h(),
                k,
            ))
        }
        _ => Err(format!("ui.rect: no such rectangle: {name}")),
    }
}
