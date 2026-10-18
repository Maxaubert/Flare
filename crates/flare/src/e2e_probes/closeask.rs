//! The close question probes (Task 24): `dialog.focus`, which of the question's buttons the keys
//! reach. Read-only: it asks the window's exported focus index and sets nothing.

use serde_json::{Value, json};

use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("dialog.focus", dialog_focus);
}

/// The name of the button the keys reach: `secondary` is Cancel (index 0), `primary` the confirm
/// button (index 1). Anything else, including a closed dialog, is `none`.
pub fn focus_name(open: bool, focused_button: i32) -> &'static str {
    match (open, focused_button) {
        (true, 1) => "primary",
        (true, 0) => "secondary",
        _ => "none",
    }
}

/// `dialog.focus`: `{ focused: "primary" | "secondary" | "none" }`.
fn dialog_focus(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let open = ctx.app.get_dialog_open();
    let name = focus_name(open, ctx.app.get_dialog_focus());
    Ok(json!({ "focused": name }))
}

#[cfg(test)]
mod tests {
    use super::focus_name;

    #[test]
    fn the_keys_name_the_button_they_reach_and_nothing_while_closed() {
        assert_eq!(focus_name(true, 1), "primary");
        assert_eq!(focus_name(true, 0), "secondary");
        assert_eq!(focus_name(true, -1), "none", "the keys left the question");
        assert_eq!(
            focus_name(false, 1),
            "none",
            "a closed question holds no focus"
        );
    }
}
