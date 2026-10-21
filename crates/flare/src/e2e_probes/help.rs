//! The command help panel probe (Task 37): `help.state`. It only looks. The model data (rows, chips,
//! the query, the count, the words a screen reader hears) is read from the `HelpBridge` the panel
//! draws from; the panel's box is the page under the title bar less the sidebar, with the sheet as
//! wide as the view says (`help-sheet-w`). The slide takes 460 ms, and the probe cannot see a frame,
//! so `slid` is true once the probe has watched the panel open for `SLIDE_MS` (a scenario polls it
//! while it waits, and polls the closed state before opening it again).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use slint::{ComponentHandle, Model};

use super::{ProbeCtx, ProbeRegistry};
use crate::{AppWindow, HelpBridge, Theme};

/// The title bar's height in css pixels (`app.slint`).
const BAR_H: f32 = 36.0;
/// How long the probe must have watched the panel open before it calls the slide over (the slide is
/// 460 ms, the view sets the focus 60 ms after its first frame, and the last of the first twelve
/// rows finishes rising at 636 ms).
const SLIDE_MS: u64 = 700;
/// The panel's padding, the field and the close button, in css pixels (`help_panel.slint`).
const PAD: f32 = 16.0;
const TITLE_H: f32 = 28.0;
const FIELD_H: f32 = 36.0;
const GAP: f32 = 10.0;
const TOP_PAD: f32 = 14.0;
const CHIP_H: f32 = 26.0;
const FOOT_H: f32 = 8.0 + 19.75 + 8.0;

/// When the probe first saw the panel open (`None` while it was seen closed).
static OPEN_SEEN: Mutex<Option<Instant>> = Mutex::new(None);

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("help.state", help_state);
}

fn hex(c: slint::Color) -> String {
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        c.red(),
        c.green(),
        c.blue(),
        c.alpha()
    )
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Value {
    json!({ "x": x, "y": y, "w": w, "h": h })
}

/// True once the panel has been seen open for `SLIDE_MS` (and resets when it is seen closed).
fn slid(open: bool, seen: &Mutex<Option<Instant>>, now: Instant) -> bool {
    let Ok(mut g) = seen.lock() else { return false };
    if !open {
        *g = None;
        return false;
    }
    let since = *g.get_or_insert(now);
    now.saturating_duration_since(since) >= Duration::from_millis(SLIDE_MS)
}

fn hair(k: f32) -> f32 {
    k.floor().max(1.0) / k
}

/// The reply of `help.state` for a window, from its bridge and its sheet. `slid` is the slide's
/// verdict (see `slid`).
pub fn state_of(app: &AppWindow, slid: bool) -> Value {
    let b = app.global::<HelpBridge>();
    let th = app.global::<Theme>();
    let k = app.window().scale_factor();
    let size = app.window().size();
    let (win_w, win_h) = (size.width as f32 / k, size.height as f32 / k);
    // The page: under the title bar, to the right of the sidebar slot when there is one.
    let side = if app.get_layout_vertical() && !app.get_sidebar_collapsed() {
        app.get_sidebar_width()
    } else {
        0.0
    };
    let page = (side, BAR_H, win_w - side, win_h - BAR_H);
    let sheet_w = app.get_help_sheet_w();
    let sheet_x = page.0 + page.2 - sheet_w;
    let hr = hair(k);
    let field_y = page.1 + TOP_PAD + TITLE_H + GAP;
    let chip_y = field_y + FIELD_H + GAP;
    let list_y = chip_y + CHIP_H + 2.0 * hr + GAP;
    let list_h = (page.1 + page.3 - FOOT_H - hr - list_y).max(0.0);

    let caps = b.get_key_caps();
    let rows: Vec<Value> = b
        .get_rows()
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let n_caps = caps.row_data(i).map_or(0, |c| c.row_count());
            let (id, variant) = r
                .key
                .split_once('#')
                .map_or((r.key.to_string(), 0), |(a, v)| {
                    (a.to_string(), v.parse::<usize>().unwrap_or(0))
                });
            json!({
                "key": r.key.to_string(),
                "id": id,
                "variant": variant,
                "name": r.name.to_string(),
                "command": r.command.to_string(),
                "header": r.header.to_string(),
                "keys": r.keys,
                "caps": n_caps,
                "danger": r.danger,
                "marked": r.active,
                "striped": r.striped,
                "copied": r.copied,
                "failed": r.failed,
            })
        })
        .collect();
    let categories: Vec<String> = b
        .get_rows()
        .iter()
        .filter(|r| !r.header.is_empty())
        .map(|r| r.header.to_string())
        .collect();
    let chips: Vec<Value> = b
        .get_chips()
        .iter()
        .map(|c| json!({ "id": c.id.to_string(), "name": c.name.to_string(), "on": c.on }))
        .collect();
    let shell = b
        .get_chips()
        .iter()
        .find(|c| c.on)
        .map(|c| c.id.to_string());
    let face = flare_theme::fonts::font_families(flare_theme::fonts::DEFAULT_FONT_ID)
        .into_iter()
        .next()
        .unwrap_or_default();
    let open = b.get_open();
    json!({
        "open": open,
        "slid": open && slid,
        "button": app.get_help_visible(),
        "on_top": app.get_help_on_top(),
        // A Slint text field of the window has the keys (the panel's field, or its edge); the find
        // bar's own field counts too, so a reader checks `find_open` beside it.
        "field_focused": app.get_find_input_focused() && !app.get_find_open(),
        "find_open": app.get_find_open(),
        "shell": shell,
        "chips": chips,
        "query": b.get_query().to_string(),
        "placeholder": b.get_placeholder().to_string(),
        "search_label": b.get_search_label().to_string(),
        "count_text": b.get_count_text().to_string(),
        "said": b.get_said().to_string(),
        "empty": b.get_empty(),
        "empty_text": b.get_empty_text().to_string(),
        "active_index": b.get_active_index(),
        "marked": b.get_rows().iter().any(|r| r.active),
        "categories": categories,
        "rows": rows,
        "viewport_y": app.get_help_viewport_y(),
        "panel_w": b.get_panel_w(),
        "sheet_w": sheet_w,
        "page": rect(page.0, page.1, page.2, page.3),
        "panel": rect(sheet_x, page.1, sheet_w, page.3),
        "close": rect(sheet_x + sheet_w - PAD - 28.0, page.1 + TOP_PAD, 28.0, TITLE_H),
        "field": rect(sheet_x + hr + PAD, field_y, sheet_w - hr - 2.0 * PAD, FIELD_H),
        "chip_row": rect(sheet_x + hr + PAD, chip_y, sheet_w - hr - 2.0 * PAD, CHIP_H + 2.0 * hr),
        "list": rect(sheet_x + hr, list_y, sheet_w - hr, list_h),
        // The left edge that resizes: 8 px wide, 4 inside and 4 outside.
        "grip": rect(sheet_x - 4.0, page.1, 8.0, page.3),
        "mono": b.get_mono().to_string(),
        "terminal_face": face,
        "ink": {
            "text": hex(th.get_text()),
            "ground": hex(th.get_side_flat()),
            "accent": hex(th.get_accent_solid()),
        },
    })
}

fn help_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let open = ctx.app.global::<HelpBridge>().get_open();
    let s = slid(open, &OPEN_SEEN, Instant::now());
    Ok(state_of(ctx.app, s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_harness::Harness;
    use crate::{HelpChip, HelpRowData};
    use slint::{ModelRc, VecModel};

    fn row(key: &str, header: &str, keys: bool, active: bool) -> HelpRowData {
        HelpRowData {
            key: key.into(),
            name: "Find big files".into(),
            command: "Get-ChildItem".into(),
            header: header.into(),
            keys,
            danger: key.starts_with("ps-delete"),
            active,
            striped: false,
            copied: false,
            failed: false,
        }
    }

    fn window() -> Harness<AppWindow> {
        Harness::new(1.0, 1000, 640, || AppWindow::new().unwrap())
    }

    #[test]
    fn the_rows_chips_and_words_come_from_the_bridge() {
        let h = window();
        let b = h.comp.global::<HelpBridge>();
        b.set_rows(ModelRc::new(VecModel::from(vec![
            row("ps-a#0", "folders", false, false),
            row("ps-delete-folder#1", "", false, true),
            row("ps-cancel#0", "", true, false),
        ])));
        b.set_chips(ModelRc::new(VecModel::from(vec![
            HelpChip {
                id: "powershell".into(),
                name: "PowerShell".into(),
                on: false,
            },
            HelpChip {
                id: "bash".into(),
                name: "Bash".into(),
                on: true,
            },
        ])));
        b.set_query("find".into());
        b.set_said("Copied ls".into());
        let s = state_of(&h.comp, false);
        assert_eq!(s["rows"].as_array().unwrap().len(), 3);
        assert_eq!(s["rows"][0]["id"], "ps-a");
        assert_eq!(s["rows"][1]["id"], "ps-delete-folder");
        assert_eq!(s["rows"][1]["variant"], 1);
        assert_eq!(s["rows"][1]["danger"], true);
        assert_eq!(s["rows"][1]["marked"], true);
        assert_eq!(s["rows"][0]["danger"], false);
        assert_eq!(s["rows"][2]["keys"], true);
        assert_eq!(s["categories"], json!(["folders"]));
        assert_eq!(s["shell"], "bash");
        assert_eq!(s["query"], "find");
        assert_eq!(s["said"], "Copied ls");
        assert_eq!(s["marked"], true);
    }

    #[test]
    fn nothing_marked_and_no_shell_is_the_other_answer() {
        let h = window();
        let b = h.comp.global::<HelpBridge>();
        b.set_rows(ModelRc::new(VecModel::from(vec![row(
            "ps-a#0", "", false, false,
        )])));
        b.set_chips(ModelRc::new(VecModel::from(vec![HelpChip {
            id: "powershell".into(),
            name: "PowerShell".into(),
            on: false,
        }])));
        let s = state_of(&h.comp, false);
        assert_eq!(s["marked"], false);
        assert_eq!(s["shell"], Value::Null);
        assert_eq!(s["categories"], json!([]));
    }

    #[test]
    fn open_is_the_bridge_and_slid_needs_the_open_panel() {
        let h = window();
        let b = h.comp.global::<HelpBridge>();
        assert_eq!(
            state_of(&h.comp, true)["slid"],
            false,
            "closed is never slid"
        );
        b.set_open(true);
        assert_eq!(state_of(&h.comp, false)["open"], true);
        assert_eq!(state_of(&h.comp, false)["slid"], false);
        assert_eq!(state_of(&h.comp, true)["slid"], true);
    }

    #[test]
    fn the_slide_is_over_after_the_probe_watched_it_open_long_enough() {
        let seen = Mutex::new(None);
        let t0 = Instant::now();
        assert!(!slid(true, &seen, t0));
        assert!(!slid(true, &seen, t0 + Duration::from_millis(SLIDE_MS - 1)));
        assert!(slid(true, &seen, t0 + Duration::from_millis(SLIDE_MS)));
        // Seen closed, the clock starts again.
        assert!(!slid(false, &seen, t0 + Duration::from_millis(900)));
        assert!(!slid(true, &seen, t0 + Duration::from_millis(901)));
        assert!(slid(
            true,
            &seen,
            t0 + Duration::from_millis(901 + SLIDE_MS)
        ));
    }

    #[test]
    fn the_sheet_hangs_on_the_right_edge_under_the_title_bar_at_every_scale() {
        for k in crate::ui_harness::SCALES {
            let h = Harness::new(k, 1000, 640, || AppWindow::new().unwrap());
            h.comp.global::<HelpBridge>().set_panel_w(620.0);
            h.advance(0);
            let s = state_of(&h.comp, true);
            let p = &s["panel"];
            let w = s["sheet_w"].as_f64().unwrap();
            assert!((p["w"].as_f64().unwrap() - w).abs() < 1e-3, "{k}");
            assert!(
                (p["x"].as_f64().unwrap() + p["w"].as_f64().unwrap() - 1000.0).abs() < 1.0,
                "flush right at {k}"
            );
            assert_eq!(p["y"], 36.0, "under the title bar at {k}");
            let page = &s["page"];
            assert!(
                (p["h"].as_f64().unwrap() - page["h"].as_f64().unwrap()).abs() < 1e-3,
                "the panel is the page's height at {k}"
            );
            let list = &s["list"];
            assert!(list["y"].as_f64().unwrap() > s["field"]["y"].as_f64().unwrap());
            assert!(
                list["y"].as_f64().unwrap() + list["h"].as_f64().unwrap()
                    <= p["y"].as_f64().unwrap() + p["h"].as_f64().unwrap(),
                "the list is inside the panel at {k}"
            );
            // The grip straddles the sheet's left edge.
            let g = &s["grip"];
            assert!(
                (g["x"].as_f64().unwrap() + 4.0 - p["x"].as_f64().unwrap()).abs() < 1e-3,
                "grip at {k}"
            );
        }
    }

    #[test]
    fn the_words_and_the_face_are_the_panels_own() {
        let h = window();
        let s = state_of(&h.comp, false);
        assert_eq!(s["mono"], "Cascadia Mono");
        assert_eq!(s["terminal_face"], s["mono"]);
        h.comp.global::<HelpBridge>().set_mono("Consolas".into());
        let s = state_of(&h.comp, false);
        assert_ne!(s["terminal_face"], s["mono"], "a different face is seen");
        assert!(s["ink"]["text"].as_str().unwrap().starts_with('#'));
    }

    #[test]
    fn escape_gives_the_keys_back_to_the_terminal_wherever_the_focus_was() {
        // The field, the edge (after a press on it, as a drag leaves it), a chip, a row.
        for (what, click) in [
            ("the field", None),
            ("the edge", Some((380.0, 300.0))),
            ("a chip", Some((420.0, 148.0))),
            ("a row", Some((500.0, 226.0))),
        ] {
            let h = Harness::new(1.0, 1000, 640, || AppWindow::new().unwrap());
            h.comp.set_front_kind(0);
            h.comp
                .window()
                .dispatch_event(slint::platform::WindowEvent::WindowActiveChanged(true));
            h.advance(0);
            let ctl = crate::help_ctl::HelpCtl::new(
                &h.comp,
                flare_settings::kv::PrefStore::memory(),
                std::rc::Rc::new(|_: &str| {}),
                std::rc::Rc::new(|| None),
                std::rc::Rc::new(|| false),
            );
            ctl.borrow_mut().toggle();
            h.advance(1);
            h.advance(900);
            assert!(
                h.comp.get_find_input_focused(),
                "{what}: the field has the keys while the panel is open"
            );
            if let Some((x, y)) = click {
                h.click(x, y);
                h.advance(50);
            }
            h.key_text(&String::from(char::from(slint::platform::Key::Escape)));
            h.advance(300);
            assert!(!ctl.borrow().is_open(), "{what}: Escape closes the panel");
            assert!(
                !h.comp.get_find_input_focused(),
                "{what}: after Escape the keys are the terminal's, not the closed panel's"
            );
        }
    }
    #[test]
    fn the_probe_file_changes_nothing() {
        let src = include_str!("help.rs");
        let body = src.split("#[cfg(test)]").next().unwrap();
        for banned in [
            ["borrow", "_mut"].concat(),
            [".set", "_"].concat(),
            ["invoke", "_"].concat(),
        ] {
            assert!(!body.contains(&banned), "the probe must not use {banned}");
        }
    }

    #[test]
    fn the_slint_source_still_says_what_the_scenario_reads_from_pixels_alone() {
        // Three facts the scenario cannot ask a pixel: a screen reader is told "Careful." for a
        // dangerous row, the command well shows the text cursor, and the edge shows the resize
        // cursor. They are Slint properties, so the source is the record.
        let src = include_str!("../../ui/settings/help_panel.slint");
        assert!(src.contains("accessible-description: root.d.danger ? \"Careful.\" : \"\""));
        assert!(src.contains("mouse-cursor: MouseCursor.text"));
        assert!(src.contains("mouse-cursor: MouseCursor.ew-resize"));
        assert!(src.contains("text: HelpBridge.said"));
    }
}
