//! Input C2: stalls, review keys and drops probes (Task 32).

use serde_json::{Value, json};

use super::{ProbeCtx, ProbeRegistry};

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("drag.state", drag_state);
}

/// What the probe answers: whether a file drag is in flight and whether the tab strip is window-drag.
fn drag_answer(in_flight: bool, strip_move_area: bool) -> Value {
    json!({ "in_flight": in_flight, "strip_move_area": strip_move_area })
}

/// PT reviewKeys #33: a drag in flight takes the strip off window-drag (`strip_move_area` false).
fn drag_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let in_flight = ctx
        .ctl
        .try_borrow()
        .map_err(|_| "drag.state: the controller is busy".to_string())?
        .drag_in_flight();
    Ok(drag_answer(in_flight, ctx.app.get_strip_has_move_area()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_names_both_halves() {
        let v = drag_answer(true, false);
        assert_eq!(v["in_flight"], true);
        assert_eq!(v["strip_move_area"], false);
    }
}
