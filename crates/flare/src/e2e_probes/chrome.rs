//! Chrome probes: the start screen, the registry write counter (Task 16), and the system menu
//! counter, the ground, the Settings tab and its rail (Task 21). Every probe only looks; nothing
//! here sets a property or borrows the controller mutably.

use std::sync::{Arc, Mutex};

use flare_settings::layout::rail_layout;
use flare_settings::pages::PAGES;
use flare_tabs::tabs::{SETTINGS_ID, TabKind};
use flare_win::verb_switch::VerbSwitch;
use serde_json::{Value, json};
use slint::ComponentHandle;

use super::{ProbeCtx, ProbeRegistry};
use crate::Theme;
use crate::appctl::system_menu_requests;

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
    r.add("window.sysmenu_requests", sysmenu_requests);
    r.add("window.ground", window_ground);
    r.add("settings.state", settings_state);
    r.add("settings.rail", settings_rail);
}

/// `{ "count": n }`: how many times the system menu was asked for (Alt+Space counts, a right click
/// never does).
pub fn sysmenu_json(count: u32) -> Value {
    json!({ "count": count })
}

fn sysmenu_requests(_ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    Ok(sysmenu_json(system_menu_requests()))
}

/// The window's ground as the theme gives it, with its relative luminance (WCAG) and whether it is
/// light (above 0.4, the scenario's line between dark and light chrome).
pub fn ground_json(rgb: (u8, u8, u8)) -> Value {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let lum = 0.2126 * lin(rgb.0) + 0.7152 * lin(rgb.1) + 0.0722 * lin(rgb.2);
    json!({
        "ground": hex(rgb.0, rgb.1, rgb.2),
        "luminance": (lum * 1000.0).round() / 1000.0,
        "light": lum > 0.4,
    })
}

fn window_ground(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let c = ctx.app.global::<Theme>().get_ground_solid();
    Ok(ground_json((c.red(), c.green(), c.blue())))
}

/// `{ open, front, page, rail }`: whether a Settings tab is open, whether it is in front, the page
/// it holds (`null` unless it is in front: the page is unmounted while another tab is in front),
/// and the rail's page ids in order.
pub fn settings_state_json(open: bool, front: bool, page: Option<&str>, rail: &[&str]) -> Value {
    json!({
        "open": open,
        "front": front,
        "page": if front { page } else { None },
        "rail": rail,
    })
}

fn settings_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let (open, front) = {
        let c = ctx.ctl.try_borrow().map_err(|_| "the controller is busy")?;
        let st = &c.model().state;
        (
            st.tabs.iter().any(|t| t.kind == TabKind::Settings),
            st.active_id.as_deref() == Some(SETTINGS_ID),
        )
    };
    let page = ctx.ctl.try_borrow().ok().and_then(|c| c.settings_page());
    let rail: Vec<&str> = PAGES.iter().map(|p| p.id).collect();
    Ok(settings_state_json(open, front, page, &rail))
}

/// A rectangle in css and device pixels (css times the scale, rounded), as `ui.rect` answers.
fn rect_value(x: f32, y: f32, w: f32, h: f32, k: f32) -> Value {
    let d = |v: f32| (v * k).round() as i64;
    json!({
        "x": x, "y": y, "w": w, "h": h,
        "dx": d(x), "dy": d(y), "dw": d(w), "dh": d(h),
    })
}

/// The rail item `id` in css pixels, inside the Settings frame `(x, y, w, h)` (the grid's box, in
/// css pixels): `rail_layout` gives its box within the frame. `None` for an unknown page.
pub fn rail_item_rect(
    id: &str,
    frame: (f32, f32, f32, f32),
    k: f32,
) -> Option<(f32, f32, f32, f32)> {
    let index = PAGES.iter().position(|p| p.id == id)?;
    let (fx, fy, fw, _fh) = frame;
    let layout = rail_layout(f64::from(fw), f64::from(k), PAGES.len());
    let (iy, ih) = *layout.items.get(index)?;
    Some((
        fx + layout.inner_x as f32,
        fy + iy as f32,
        layout.inner_w as f32,
        ih as f32,
    ))
}

fn settings_rail(ctx: &ProbeCtx, arg: &Value) -> Result<Value, String> {
    let id = arg
        .get("id")
        .and_then(Value::as_str)
        .ok_or("settings.rail needs an id")?;
    let k = ctx.app.window().scale_factor();
    let frame = (
        ctx.app.get_grid_left(),
        ctx.app.get_grid_top(),
        ctx.app.get_grid_phys_width() as f32 / k,
        ctx.app.get_grid_phys_height() as f32 / k,
    );
    let (x, y, w, h) =
        rail_item_rect(id, frame, k).ok_or_else(|| format!("settings.rail: no page {id}"))?;
    Ok(rect_value(x, y, w, h, k))
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

/// The icon's two flames at their first stops (assets/icon/flare-color.svg).
const MARK_ORANGE: &str = "#ff4800";
const MARK_GOLD: &str = "#ffc500";

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
        // The theme's accent (the start screen's mark is no longer drawn in it).
        "accent": hex(accent.0, accent.1, accent.2),
        // The mark is the icon's own two tones (owner, 2026-10-22): the orange flame and the golden one over it.
        "mark_colour": MARK_ORANGE,
        "mark_gold": MARK_GOLD,
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
        assert_eq!(v["accent"], "#fe8f34");
        assert_eq!(v["mark_colour"], "#ff4800");
        assert_eq!(v["mark_gold"], "#ffc500");
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
        assert_eq!(
            r.names(),
            [
                "registry.writes",
                "settings.rail",
                "settings.state",
                "start.state",
                "window.ground",
                "window.sysmenu_requests",
            ]
        );
    }

    #[test]
    fn the_system_menu_answer_is_a_count() {
        assert_eq!(sysmenu_json(0)["count"], 0);
        assert_eq!(sysmenu_json(1)["count"], 1);
    }

    #[test]
    fn a_light_ground_is_light_and_a_dark_one_is_not() {
        // The `theme` scenario's line: the luminance above 0.4 is light chrome.
        let paper = ground_json((0xf6, 0xf1, 0xe7));
        assert_eq!(paper["light"], true);
        assert_eq!(paper["ground"], "#f6f1e7");
        assert!(paper["luminance"].as_f64().unwrap() > 0.4);
        let dark = ground_json((0x1e, 0x1e, 0x2e));
        assert_eq!(dark["light"], false);
        assert!(dark["luminance"].as_f64().unwrap() < 0.1);
    }

    #[test]
    fn a_settings_page_is_reported_only_while_its_tab_is_in_front() {
        let rail = ["appearance", "terminal", "dictation"];
        let front = settings_state_json(true, true, Some("dictation"), &rail);
        assert_eq!(front["open"], true);
        assert_eq!(front["page"], "dictation");
        assert_eq!(front["rail"], json!(rail));
        // Another tab in front: the page is kept but not mounted, so it reads as null.
        let behind = settings_state_json(true, false, Some("dictation"), &rail);
        assert!(behind["page"].is_null());
        assert_eq!(behind["open"], true);
        let closed = settings_state_json(false, false, None, &rail);
        assert_eq!(closed["open"], false);
        assert!(closed["page"].is_null());
    }

    #[test]
    fn every_rail_item_sits_inside_the_settings_frame_at_every_scale() {
        // The eight scales of Appendix E: the rail's boxes stay in the frame at each.
        let frame = (0.0_f32, 36.0, 1000.0, 600.0);
        for k in [1.0_f32, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0] {
            let mut last_y = f32::MIN;
            for p in PAGES.iter() {
                let (x, y, w, h) = rail_item_rect(p.id, frame, k)
                    .unwrap_or_else(|| panic!("{} has a rail item at {k}", p.id));
                assert!(x >= 0.0 && x + w <= 1000.0, "{} across at {k}", p.id);
                assert!(y >= 36.0 && y + h <= 636.0, "{} down at {k}", p.id);
                assert!(y > last_y, "the rail runs top to bottom at {k}");
                last_y = y;
            }
        }
        assert!(rail_item_rect("no-such-page", frame, 1.0).is_none());
    }
}
