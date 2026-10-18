//! Settings A probes (Task 33): the rows the open page draws, the find field and its results, and
//! the row catalogue. Read-only: every answer is read from the `SettingsBridge` global the
//! controller writes, or from `flare_settings::rows`, never set. Nothing here runs without --e2e,
//! because `e2e_glue::install` is the only caller.

use flare_settings::layout::{RailLayout, about_y, rail_layout};
use flare_settings::pages::PAGES;
use flare_settings::rows::{Source, all_rows};
use serde_json::{Value, json};
use slint::{ComponentHandle, Model};

use super::{ProbeCtx, ProbeRegistry};
use crate::SettingsBridge;

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("settings.rows", settings_rows);
    r.add("settings.search", settings_search);
    r.add("settings.rail_end", settings_rail_end);
    r.add("settings.find_rect", settings_find_rect);
}

/// The Settings frame's rail layout and the frame's origin, in css px, as the controller lays it
/// out (the same inputs `settings.rail` uses).
fn frame_rail(ctx: &ProbeCtx) -> (f64, f64, f64, RailLayout) {
    let k = f64::from(ctx.app.window().scale_factor());
    let fx = ctx.app.get_grid_left() as f64;
    let fy = ctx.app.get_grid_top() as f64;
    let fw = ctx.app.get_grid_phys_width() as f64 / k;
    let fh = ctx.app.get_grid_phys_height() as f64 / k;
    (fx, fy, fh, rail_layout(fw, k, PAGES.len()))
}

/// `settings.rail_end`: the rectangle (css px, window-relative as `ui.rect` answers) of the About
/// item, which the rail pins to its bottom. `settings.rail` places items by index and does not
/// know the pin, so the bottom item's box comes from the same layout the controller uses.
fn settings_rail_end(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let (fx, fy, fh, rail) = frame_rail(ctx);
    let item_h = rail.items.last().map_or(0.0, |i| i.1);
    let y = about_y(&rail, fh);
    Ok(json!({
        "x": fx + rail.inner_x,
        "y": fy + y,
        "w": rail.inner_w,
        "h": item_h,
    }))
}

/// `settings.find_rect`: the Find field's box (css px) in the wide frame, where the field sits at
/// the top of the rail. The narrow frame folds the field into a magnifier, which is not probed.
fn settings_find_rect(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let (fx, fy, _fh, rail) = frame_rail(ctx);
    if rail.narrow {
        return Err("settings.find_rect: the narrow frame folds the field into a magnifier".into());
    }
    Ok(json!({
        "x": fx + rail.inner_x,
        "y": fy + rail.find_y,
        "w": rail.inner_w,
        "h": rail.find_h,
    }))
}

/// The name a catalogue row's source answers with.
fn source_name(s: Source) -> &'static str {
    match s {
        Source::Terminal => "terminal",
        Source::Marks => "marks",
        Source::Dictation => "dictation",
        Source::Help => "help",
        Source::Diagnostics => "diagnostics",
        Source::App => "app",
    }
}

/// `{ count, rows: [{ id, label, section, page, source, only_nvidia }] }`: every row of every
/// table, in draw order, as `flare_settings::rows` holds them. Pure, so a test needs no window.
pub fn catalogue_json() -> Value {
    let rows: Vec<Value> = all_rows()
        .map(|r| {
            json!({
                "id": r.id,
                "label": r.label,
                "section": r.section,
                "page": r.page,
                "source": source_name(r.source),
                "only_nvidia": r.only_nvidia,
            })
        })
        .collect();
    json!({ "count": rows.len(), "rows": rows })
}

/// `settings.rows`: with `{ "catalogue": true }`, the catalogue above. Otherwise the open page:
/// its id (null unless Settings is in front), its sections (each one panel, with the row ids in
/// draw order) and its rows (`y` and `h` inside the panel, `flash` 0 to 1, `block` and `kind`),
/// plus the pane's scroll and height and whether the rail is in its narrow (icons only) form.
fn settings_rows(ctx: &ProbeCtx, arg: &Value) -> Result<Value, String> {
    if arg.get("catalogue").and_then(Value::as_bool) == Some(true) {
        return Ok(catalogue_json());
    }
    let b = ctx.app.global::<SettingsBridge>();
    let page = ctx.ctl.try_borrow().ok().and_then(|c| c.settings_page());
    let mut sections = Vec::new();
    let mut rows = Vec::new();
    for s in b.get_sections().iter() {
        let mut ids = Vec::new();
        for r in s.rows.iter() {
            let id = r.id.to_string();
            ids.push(id.clone());
            rows.push(json!({
                "id": id,
                "section": s.id.to_string(),
                "label": r.label.to_string(),
                "sub": r.sub.to_string(),
                "block": format!("{:?}", r.block).to_lowercase(),
                "kind": format!("{:?}", r.kind).to_lowercase(),
                "y": r.y,
                "h": r.h,
                "flash": r.flash,
            }));
        }
        sections.push(json!({
            "id": s.id.to_string(),
            "title": s.title.to_string(),
            "panel_y": s.panel_y,
            "panel_h": s.panel_h,
            // A section is one panel: the view draws one SetPanel per section.
            "panels": 1,
            "ids": ids,
        }));
    }
    let chosen: Vec<String> = b
        .get_rail()
        .iter()
        .filter(|i| i.chosen)
        .map(|i| i.id.to_string())
        .collect();
    // The theme wall: its card count and how many rows of cards it wraps into (distinct `y`).
    let cards: Vec<f32> = b.get_wall_cards().iter().map(|c| c.y).collect();
    let mut wall_rows: Vec<i64> = cards.iter().map(|y| (y * 100.0).round() as i64).collect();
    wall_rows.sort_unstable();
    wall_rows.dedup();
    Ok(json!({
        "page": page,
        "count": rows.len(),
        "sections": sections,
        "rows": rows,
        "rail_chosen": chosen,
        "narrow": b.get_narrow(),
        "scroll_y": b.get_scroll_y(),
        "pane_h": b.get_pane_h(),
        "wall": { "cards": cards.len(), "rows": wall_rows.len() },
    }))
}

/// `settings.search`: the find field's text, whether a search is on, its status line and the
/// "Nothing matches" line, the result list (id, label, where, selected), the cursor (-1 for none),
/// and `owns_escape` (true while the field holds text, the same rule as `FindState`).
fn settings_search(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let b = ctx.app.global::<SettingsBridge>();
    let query = b.get_find_text().to_string();
    let hits: Vec<Value> = b
        .get_hits()
        .iter()
        .map(|h| {
            json!({
                "id": h.id.to_string(),
                "label": h.label.to_string(),
                "where": h.r#where.to_string(),
                "selected": h.selected,
            })
        })
        .collect();
    let rail_chosen: Vec<String> = b
        .get_rail()
        .iter()
        .filter(|i| i.chosen)
        .map(|i| i.id.to_string())
        .collect();
    Ok(json!({
        "owns_escape": !query.is_empty(),
        "query": query,
        "searching": b.get_searching(),
        "status": b.get_status_text().to_string(),
        "nothing": b.get_nothing_text().to_string(),
        "hit_cursor": b.get_hit_cursor(),
        "hits": hits,
        "rail_chosen": rail_chosen,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_lists_every_row_once_with_its_section_and_page() {
        let c = catalogue_json();
        let rows = c["rows"].as_array().expect("rows");
        assert_eq!(c["count"].as_u64(), Some(rows.len() as u64));
        assert!(rows.len() >= 39, "the 39 rows of the draw order ({})", rows.len());
        let mut ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "no row id appears twice");
        for r in rows {
            assert!(!r["section"].as_str().unwrap_or("").is_empty());
            assert!(!r["page"].as_str().unwrap_or("").is_empty());
        }
    }

    #[test]
    fn the_catalogue_names_each_source_and_the_gpu_row_is_nvidia_only() {
        let c = catalogue_json();
        let rows = c["rows"].as_array().unwrap();
        let sources: Vec<&str> = rows.iter().map(|r| r["source"].as_str().unwrap()).collect();
        for s in ["terminal", "marks", "dictation", "help", "diagnostics", "app"] {
            assert!(sources.contains(&s), "the {s} source is listed");
        }
        let gpu = rows.iter().find(|r| r["id"] == "dictation-gpu").unwrap();
        assert_eq!(gpu["only_nvidia"], json!(true));
    }
}
