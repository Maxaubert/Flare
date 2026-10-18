//! Settings D probes (Task 36): the theme's chrome tokens as the window paints them, and the window
//! border the controller computes for an edge word. Read-only: every answer is read from the `Theme`
//! global (which the controller writes) or computed from pure functions (`window_edge::edge_for`),
//! and nothing here sets a property. Nothing runs without --e2e, because `e2e_glue::install` is the
//! only caller.
//!
//! Under --e2e the DWM border is never applied (`material_ctl`: `edge_enabled = !e2e`), so no call is
//! recorded for the harness to read. `window.edge` answers the border the controller WOULD set for
//! the ground and edge word it is given: the same function the controller calls, so a wrong edge
//! table shows here even though the call itself is off.

use flare_theme::edges::WindowEdges;
use flare_win::window_edge::edge_for;
use serde_json::{json, Value};
use slint::ComponentHandle;

use super::{ProbeCtx, ProbeRegistry};
use crate::Theme;

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("theme.tokens", theme_tokens);
    r.add("window.edge", window_edge);
}

/// A colour as `#rrggbbaa`, the form the other probes answer in.
fn hex8(c: slint::Color) -> String {
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        c.red(),
        c.green(),
        c.blue(),
        c.alpha()
    )
}

/// `theme.tokens`: the chrome tokens the window paints with, each `#rrggbbaa`. `divider` is the
/// title bar's and the rail's rule, `line` the list's rule and the rule between rows, `sel_bg` the
/// on switch's fill, `switch_knob` its knob, `pressed` a selected segment's fill. `edge` is the
/// window border Slint holds (transparent under --e2e, see the module docs).
fn theme_tokens(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let t = ctx.app.global::<Theme>();
    Ok(json!({
        "accent": hex8(t.get_accent()),
        "accent_solid": hex8(t.get_accent_solid()),
        "accent_hi": hex8(t.get_accent_hi()),
        "sel_bg": hex8(t.get_sel_bg()),
        "switch_knob": hex8(t.get_switch_knob()),
        "on_accent": hex8(t.get_on_accent()),
        "control": hex8(t.get_control()),
        "pressed": hex8(t.get_pressed()),
        "divider": hex8(t.get_divider()),
        "line": hex8(t.get_line()),
        "ground": hex8(t.get_ground()),
        "ground_solid": hex8(t.get_ground_solid()),
        "edge": hex8(t.get_edge()),
        "light": t.get_light(),
    }))
}

/// `window.edge { edges }`: the border `edge_for` gives the solid ground in force for that edge word
/// (`#rrggbb`), or null for `none`. A word that is not one of the four is an error.
fn window_edge(ctx: &ProbeCtx, arg: &Value) -> Result<Value, String> {
    let word = arg
        .get("edges")
        .and_then(Value::as_str)
        .ok_or("window.edge needs an edges word")?;
    let edges = WindowEdges::from_word(word)
        .ok_or_else(|| format!("window.edge: {word:?} is not an edge word"))?;
    let g = ctx.app.global::<Theme>().get_ground_solid();
    let hex = format!("#{:02x}{:02x}{:02x}", g.red(), g.green(), g.blue());
    Ok(json!(edge_for(&hex, edges)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex8_writes_all_four_channels_in_lower_case() {
        let c = slint::Color::from_argb_u8(0x80, 0xE0, 0x7A, 0x2F);
        assert_eq!(hex8(c), "#e07a2f80");
    }

    #[test]
    fn the_edge_words_the_probe_takes_are_the_four_the_row_offers() {
        for w in ["none", "faint", "hairline", "solid"] {
            assert!(WindowEdges::from_word(w).is_some(), "{w}");
        }
        assert!(WindowEdges::from_word("dotted").is_none());
    }

    #[test]
    fn no_edges_is_no_border_for_the_probe_as_for_the_controller() {
        assert_eq!(edge_for("#171717", WindowEdges::None), None);
        assert!(edge_for("#171717", WindowEdges::Solid).is_some());
    }
}
