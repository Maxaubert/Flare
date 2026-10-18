//! Chrome probes: the start screen and the registry write counter (Task 16). Task 21 adds the rest.
//! Every probe only looks; nothing here sets a property or borrows the controller mutably.

use std::sync::{Arc, Mutex};

use flare_win::verb_switch::VerbSwitch;
use serde_json::{Value, json};
use slint::ComponentHandle;

use super::{ProbeCtx, ProbeRegistry};
use crate::Theme;

/// The words on the start screen's first row (a literal in `start.slint`).
const NEW_ROW_LABEL: &str = "New terminal";
/// The start screen's footer height, the mark block's parts and the row height, in css pixels
/// (`start.slint`: `foot-h`, `84 + 16 + 28 + 40`, the nav's `168` offset, the 36 px row).
const FOOT_H: f32 = 36.0;
const BLOCK_ABOVE_NAV: f32 = 84.0 + 16.0 + 28.0 + 40.0;
const NAV_TOP: f32 = 168.0;
const ROW_H: f32 = 36.0;

/// The verb switch whose writes the `registry.writes` probe counts (set once by the window glue).
static VERB: Mutex<Option<(Arc<VerbSwitch>, String)>> = Mutex::new(None);

/// Hands the probes the verb switch of this run. Called once, by the window glue, under `--e2e`.
/// `root` is the registry root the switch writes under (a sandbox root under `--e2e`).
pub fn note_verb_switch(sw: Arc<VerbSwitch>, root: String) {
    if let Ok(mut g) = VERB.lock() {
        *g = Some((sw, root));
    }
}

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("start.state", start_state);
    r.add("registry.writes", registry_writes);
}

/// `Px.snap(l)`: a length rounded to whole device pixels, in css pixels.
fn snap(l: f32, k: f32) -> f32 {
    (l * k).round() / k
}

/// The nav's height for `places` folder rows (`start.slint`: `nav-h`).
fn nav_height(places: usize) -> f32 {
    if places > 0 {
        112.0 + places as f32 * 38.0
    } else {
        93.0
    }
}

/// The centre of the New row in window-client css pixels, for a start screen at `(x, y)` that is
/// `w` by `h` and shows `places` folder rows, at display scale `k`.
pub fn new_row_centre(x: f32, y: f32, w: f32, h: f32, places: usize, k: f32) -> (f32, f32) {
    let block = BLOCK_ABOVE_NAV + nav_height(places);
    let top = snap((h - FOOT_H - block) / 2.0, k);
    (x + w / 2.0, y + top + NAV_TOP + ROW_H / 2.0)
}

fn hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// The reply of `start.state`, from plain numbers (so a test needs no window).
pub fn start_state_json(
    shown: bool,
    accent: (u8, u8, u8),
    places: usize,
    rect: (f32, f32, f32, f32),
    k: f32,
) -> Value {
    let (x, y, w, h) = rect;
    let (cx, cy) = new_row_centre(x, y, w, h, places, k);
    json!({
        "shown": shown,
        // The mark is drawn in `Theme.accent-solid`, so the accent is the mark's colour.
        "mark_colour": hex(accent.0, accent.1, accent.2),
        "button_label": NEW_ROW_LABEL,
        "buttons": [NEW_ROW_LABEL],
        "places": places,
        "new_centre": { "x": cx, "y": cy },
    })
}

fn start_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    use slint::Model;
    let a = ctx.app;
    let c = a.global::<Theme>().get_accent_solid();
    Ok(start_state_json(
        a.get_front_kind() == 2,
        (c.red(), c.green(), c.blue()),
        a.get_places().row_count(),
        (
            a.get_start_x(),
            a.get_start_y(),
            a.get_start_w(),
            a.get_start_h(),
        ),
        a.window().scale_factor(),
    ))
}

/// The count of registry writes the verb switch attempted this session.
/// Under `--e2e` the switch writes only into its sandbox root, so a count above zero is allowed
/// when the root is one (`sandboxed`); the scenario judges the pair.
pub fn writes_json(sw: Option<(&VerbSwitch, &str)>) -> Value {
    json!({
        "count": sw.map_or(0, |(s, _)| s.writes()),
        "root": sw.map_or("", |(_, r)| r),
    })
}

fn registry_writes(_ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let g = VERB.lock().map_err(|_| "the verb switch is locked")?;
    Ok(writes_json(
        g.as_ref().map(|(s, r)| (s.as_ref(), r.as_str())),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flare_win::registry::FakeRegistry;

    #[test]
    fn the_new_row_centre_sits_in_the_middle_of_the_row_and_the_screen() {
        // PT-free: the same sums as ui_tests/flows.rs `start_new_center` (no places).
        let (x, y) = new_row_centre(0.0, 36.0, 1000.0, 604.0, 0, 1.0);
        assert_eq!(x, 500.0);
        let block = 84.0 + 16.0 + 28.0 + 40.0 + 93.0;
        let top = ((604.0 - 36.0 - block) / 2.0_f32).round();
        assert_eq!(y, 36.0 + top + 168.0 + 18.0);
    }

    #[test]
    fn places_push_the_block_up_and_the_row_stays_inside_the_screen() {
        for k in [1.0_f32, 1.25, 1.5, 2.0, 2.25, 3.0] {
            let (_, none) = new_row_centre(0.0, 36.0, 800.0, 500.0, 0, k);
            let (_, many) = new_row_centre(0.0, 36.0, 800.0, 500.0, 5, k);
            assert!(many < none, "more rows lift the block at {k}");
            assert!(none > 36.0 && none < 36.0 + 500.0 - FOOT_H, "inside at {k}");
        }
    }

    #[test]
    fn the_start_state_names_the_mark_colour_and_the_button() {
        let v = start_state_json(true, (0xfe, 0x8f, 0x34), 0, (0.0, 36.0, 800.0, 500.0), 1.0);
        assert_eq!(v["shown"], true);
        assert_eq!(v["mark_colour"], "#fe8f34");
        assert_eq!(v["button_label"], "New terminal");
        assert_eq!(v["buttons"], json!(["New terminal"]));
        assert!(v["new_centre"]["x"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn the_write_counter_is_zero_without_a_switch_and_counts_a_real_one() {
        assert_eq!(writes_json(None)["count"], 0);
        let dir = tempfile::tempdir().unwrap();
        let sw = VerbSwitch::new(
            Arc::new(FakeRegistry::new()),
            r"Software\Classes".into(),
            false,
            None,
            dir.path().join("off"),
        );
        let v = writes_json(Some((&sw, r"Software\FlareE2E\x\Classes")));
        assert_eq!(v["count"], 0);
        assert_eq!(v["root"], r"Software\FlareE2E\x\Classes");
    }

    #[test]
    fn the_chrome_probes_are_registered() {
        let mut r = ProbeRegistry::new();
        register(&mut r);
        assert_eq!(r.names(), ["registry.writes", "start.state"]);
    }
}
