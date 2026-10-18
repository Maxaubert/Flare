//! Settings B probes (Task 34): the theme wall and its cards, the theme editor and its wells, the
//! Save changes buttons, the switch question, the colour picker's state, a row's rectangle and the
//! window's accent. Read-only: every answer comes from the `SettingsBridge` and `Theme` globals,
//! and nothing here sets a property. Rectangles are css pixels in window-client coordinates, as
//! `ui.rect` gives them. Nothing runs without --e2e, because `e2e_glue::install` is the only caller.
//!
//! The geometry is the same arithmetic the `.slint` files use (`view.slint`, `wall.slint`,
//! `colour.slint`, `controls.slint`), written out here so a probe can say where a control is.

use flare_settings::layout::{Metrics, is_narrow, rail_width};
use serde_json::{Value, json};
use slint::{ComponentHandle, Model};

use super::{ProbeCtx, ProbeRegistry};
use crate::{SettingsBridge, Theme};

/// The wall's padding and its top inset inside a wall row (`ThemeWallView`).
const WALL_PAD_X: f64 = 16.0;
const WALL_TOP: f64 = 14.0;
/// A wall card (`ThemeCard`) and its pencil.
const CARD_W: f64 = 196.0;
const CARD_H: f64 = 124.0;
const PENCIL: f64 = 20.0;
/// A colour row's swatch and its right padding (`SetRowFrame`).
const SWATCH_W: f64 = 36.0;
const SWATCH_H: f64 = 24.0;
const ROW_PAD_RIGHT: f64 = 16.0;
/// The editor's label column and the gap after it, and the editor's title and button heights.
const LABEL_W: f64 = 86.0;
const LABEL_GAP: f64 = 8.0;
const TITLE_H: f64 = 20.0;

/// The buttons of the switch question, in the order `AskThree` numbers them (0, 1, 2).
const ASK_BUTTONS: [&str; 3] = ["Cancel", "Discard", "Save as Custom"];

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("settings.theme", settings_theme);
    r.add("settings.colours", settings_colours);
    r.add("settings.row", settings_row);
}

/// A css rectangle.
#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Rect {
    fn right(self) -> f64 {
        self.x + self.w
    }

    fn bottom(self) -> f64 {
        self.y + self.h
    }

    fn json(self) -> Value {
        json!({ "x": self.x, "y": self.y, "w": self.w, "h": self.h })
    }
}

/// Snaps a css length to whole device pixels at scale `k`.
fn snap(l: f64, k: f64) -> f64 {
    (l * k).round() / k
}

/// `Px.hair()`: `max(1, floor(k))` device pixels, in css px.
fn hair(k: f64) -> f64 {
    k.floor().max(1.0) / k
}

/// Where the Settings frame is in the window, how far its pane is scrolled, and its scale.
#[derive(Clone, Copy)]
struct Frame {
    left: f64,
    top: f64,
    w: f64,
    h: f64,
    k: f64,
    scroll: f64,
}

impl Frame {
    fn pane_x(&self) -> f64 {
        rail_width(self.w)
    }

    fn pad(&self) -> f64 {
        let m = Metrics::reference();
        snap(
            if is_narrow(self.w) {
                m.pad_x_narrow
            } else {
                m.pad_x
            },
            self.k,
        )
    }

    fn content_w(&self) -> f64 {
        let m = Metrics::reference();
        (m.column_max.min(self.w - self.pane_x()) - 2.0 * self.pad()).max(0.0)
    }

    /// A panel at content-y `panel_y`, in window coordinates (the scroll applied).
    fn panel(&self, panel_y: f64, panel_h: f64) -> Rect {
        Rect {
            x: self.left + self.pane_x() + self.pad(),
            y: self.top + panel_y - self.scroll,
            w: self.content_w(),
            h: panel_h,
        }
    }

    /// A row inside a panel: the view adds one hairline to `row_y` and takes one off each side.
    fn row(&self, panel: Rect, row_y: f64, row_h: f64) -> Rect {
        let hr = hair(self.k);
        Rect {
            x: panel.x + hr,
            y: panel.y + hr + row_y,
            w: panel.w - 2.0 * hr,
            h: row_h,
        }
    }

    /// True when `r` lies inside the pane on screen.
    fn in_pane(&self, r: Rect) -> bool {
        r.y >= self.top && r.bottom() <= self.top + self.h
    }
}

fn frame(ctx: &ProbeCtx) -> Frame {
    let k = f64::from(ctx.app.window().scale_factor());
    Frame {
        left: f64::from(ctx.app.get_grid_left()),
        top: f64::from(ctx.app.get_grid_top()),
        w: f64::from(ctx.app.get_grid_phys_width()) / k,
        h: f64::from(ctx.app.get_grid_phys_height()) / k,
        k,
        scroll: f64::from(ctx.app.global::<SettingsBridge>().get_scroll_y()),
    }
}

fn css_window(ctx: &ProbeCtx) -> (f64, f64) {
    let s = ctx.app.window().size();
    let k = f64::from(ctx.app.window().scale_factor());
    (f64::from(s.width) / k, f64::from(s.height) / k)
}

/// A colour row's swatch: right-aligned in its row, 16 px in, centred vertically.
fn colour_swatch(row: Rect) -> Rect {
    Rect {
        x: row.right() - ROW_PAD_RIGHT - SWATCH_W,
        y: row.y + (row.h - SWATCH_H) / 2.0,
        w: SWATCH_W,
        h: SWATCH_H,
    }
}

/// A colour row's code box, left of the swatch (172 px when wide, 76 px otherwise).
fn colour_code(row: Rect, wide: bool) -> Rect {
    let sw = colour_swatch(row);
    let w = if wide { 172.0 } else { 76.0 };
    Rect {
        x: sw.x - 6.0 - w,
        y: sw.y + 1.0,
        w,
        h: 22.0,
    }
}

/// The wall's own box inside its row.
fn wall_box(row: Rect) -> Rect {
    Rect {
        x: row.x + WALL_PAD_X,
        y: row.y + WALL_TOP,
        w: row.w - 2.0 * WALL_PAD_X,
        h: 0.0,
    }
}

/// One card and its pencil, at the card's offset inside the wall.
fn wall_card(wall: Rect, cx: f64, cy: f64, k: f64) -> (Rect, Rect) {
    let card = Rect {
        x: wall.x + cx,
        y: wall.y + cy,
        w: CARD_W,
        h: CARD_H + 2.0 * hair(k),
    };
    let pencil = Rect {
        x: card.x + snap(card.w - 30.0, k),
        y: card.y + snap(card.h - 26.0, k),
        w: PENCIL,
        h: PENCIL,
    };
    (card, pencil)
}

/// The theme editor's layout (`ThemeEditor` in `wall.slint`): its box, whether the preview sits
/// beside the wells, each well's cell, swatch and the Save and Cancel buttons.
struct Editor {
    box_: Rect,
    beside: bool,
    cells: Vec<Rect>,
    swatches: Vec<Rect>,
    save: Rect,
    cancel: Rect,
    card: Rect,
}

fn editor_geometry(win_w: f64, win_h: f64, k: f64, n: usize, any_wide: bool) -> Editor {
    let field_w = if any_wide { 214.0 } else { 118.0 };
    let col_w = LABEL_W + LABEL_GAP + field_w;
    let grid_w = 2.0 * col_w + 24.0;
    let rows = n.div_ceil(2) as f64;
    let grid_h = if rows > 0.0 {
        rows * 24.0 + (rows - 1.0) * 6.0
    } else {
        0.0
    };
    let card_w = 196.0;
    let card_h = 124.0 + 2.0 * hair(k);
    let avail = win_w - 48.0;
    let beside = avail - 40.0 >= grid_w + 24.0 + card_w;
    let inner_h = grid_h + if beside { 0.0 } else { 24.0 + card_h };
    let content_w = if beside {
        grid_w + 24.0 + card_w
    } else {
        grid_w.max(card_w)
    };
    let content_h = 20.0 + TITLE_H + 12.0 + inner_h + 16.0 + 32.0 + 20.0;
    let box_w = snap(avail.min(content_w + 40.0), k);
    let box_h = snap(content_h.min(win_h * 0.85), k);
    let box_x = snap((win_w - box_w) / 2.0, k);
    let box_y = snap((win_h - box_h) / 2.0, k);
    let grid_x = box_x + 20.0;
    let grid_y = box_y + 20.0 + TITLE_H + 12.0;
    let cell = |j: usize| {
        let col = (j % 2) as f64;
        let row = (j / 2) as f64;
        let cx = snap(20.0 + col * (col_w + 24.0), k);
        let cy = snap(20.0 + TITLE_H + 12.0 + row * 30.0, k);
        (box_x + cx, box_y + cy, col_w)
    };
    let cells = (0..n)
        .map(|j| {
            let (x, y, w) = cell(j);
            Rect { x, y, w, h: 24.0 }
        })
        .collect();
    let swatches = (0..n)
        .map(|j| {
            let (x, y, w) = cell(j);
            Rect {
                x: x + w - SWATCH_W,
                y,
                w: SWATCH_W,
                h: SWATCH_H,
            }
        })
        .collect();
    let button_y = grid_y + inner_h + 16.0;
    Editor {
        box_: Rect {
            x: box_x,
            y: box_y,
            w: box_w,
            h: box_h,
        },
        beside,
        cells,
        swatches,
        save: Rect {
            x: grid_x,
            y: button_y,
            w: 100.0,
            h: 32.0,
        },
        cancel: Rect {
            x: grid_x + 108.0 + 8.0,
            y: button_y,
            w: 50.0,
            h: 32.0,
        },
        card: Rect {
            x: grid_x + if beside { grid_w + 24.0 } else { 0.0 },
            y: grid_y + if beside { 0.0 } else { grid_h + 24.0 },
            w: card_w,
            h: card_h,
        },
    }
}

fn hex8(c: slint::Color) -> String {
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        c.red(),
        c.green(),
        c.blue(),
        c.alpha()
    )
}

/// Every Save changes row of the page in front (a row of kind `save`, drawn right-aligned in its
/// row): whether it is lit, where to click it (inside the button, 10 px in from its right edge,
/// clear of the label), and whether the row is on screen.
fn saves(ctx: &ProbeCtx, f: &Frame) -> Vec<Value> {
    let b = ctx.app.global::<SettingsBridge>();
    let mut out = Vec::new();
    for s in b.get_sections().iter() {
        let panel = f.panel(f64::from(s.panel_y), f64::from(s.panel_h));
        for r in s.rows.iter() {
            if format!("{:?}", r.kind).to_lowercase() != "save" {
                continue;
            }
            let row = f.row(panel, f64::from(r.y), f64::from(r.h));
            let point = json!({
                "x": row.right() - ROW_PAD_RIGHT - 10.0,
                "y": row.y + row.h / 2.0,
            });
            out.push(json!({
                "section": s.id.to_string(),
                "id": r.id.to_string(),
                "dirty": r.save_dirty,
                "title": r.save_title.to_string(),
                "point": point,
                "visible": f.in_pane(row),
            }));
        }
    }
    out
}

/// `settings.theme`: the theme wall (its cards, the one being edited), the theme editor (open,
/// its wells, its Save and Cancel), the colour picker (open, its colour), the switch question
/// (open, its buttons in order) and every section's Save changes (dirty, lit, where to click).
fn settings_theme(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let b = ctx.app.global::<SettingsBridge>();
    let f = frame(ctx);
    let (ww, wh) = css_window(ctx);

    // The wall: the row whose block is the wall, on the page in front.
    let mut wall_row: Option<Rect> = None;
    for s in b.get_sections().iter() {
        for r in s.rows.iter() {
            if format!("{:?}", r.block).to_lowercase() == "wall" {
                let panel = f.panel(f64::from(s.panel_y), f64::from(s.panel_h));
                wall_row = Some(f.row(panel, f64::from(r.y), f64::from(r.h)));
            }
        }
    }
    let wall_json = match wall_row {
        None => Value::Null,
        Some(row) => {
            let wall = wall_box(row);
            let expanded = b.get_wall_expanded();
            let shown_h = if expanded {
                f64::from(b.get_wall_full_h())
            } else {
                f64::from(b.get_wall_clip_h())
            };
            let cards: Vec<Value> = b
                .get_wall_cards()
                .iter()
                .map(|c| {
                    let (card, pencil) = wall_card(wall, f64::from(c.x), f64::from(c.y), f.k);
                    // A card is hit only while the clip shows it.
                    let shown = f64::from(c.y) + CARD_H <= shown_h + 8.0;
                    json!({
                        "id": c.id.to_string(),
                        "name": c.name.to_string(),
                        "on": c.on,
                        "edit": c.edit,
                        "rect": card.json(),
                        "pencil": pencil.json(),
                        "shown": shown,
                        "ground": hex8(c.bg),
                        "ink": hex8(c.fg),
                    })
                })
                .collect();
            json!({
                "row": row.json(),
                "expanded": expanded,
                "cards": cards,
            })
        }
    };

    // The editor, as the view lays it out for the wells it holds.
    let wells: Vec<_> = b.get_editor_wells().iter().collect();
    let any_wide = wells.iter().any(|w| w.wide);
    let geo = editor_geometry(ww, wh, f.k, wells.len(), any_wide);
    let wells_json: Vec<Value> = wells
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let cell = geo.cells.get(i).copied().unwrap_or(geo.box_);
            let sw = geo.swatches.get(i).copied().unwrap_or(geo.box_);
            // The text field runs from the label's column to the swatch; a click at its left part
            // lands in it.
            let field = Rect {
                x: cell.x + LABEL_W + LABEL_GAP,
                y: cell.y,
                w: cell.w - LABEL_W - LABEL_GAP,
                h: 24.0,
            };
            json!({
                "key": w.key.to_string(),
                "label": w.label.to_string(),
                "text": w.text.to_string(),
                "colour": hex8(w.colour),
                "wide": w.wide,
                "open": w.open,
                "swatch": sw.json(),
                "field": field.json(),
            })
        })
        .collect();
    let preview = b.get_editor_preview();
    let editor = json!({
        "open": b.get_editor_open(),
        "box": geo.box_.json(),
        "beside": geo.beside,
        "wells": wells_json,
        "save": geo.save.json(),
        "cancel": geo.cancel.json(),
        "card": geo.card.json(),
        "preview": { "id": preview.id.to_string(), "bg": hex8(preview.bg), "fg": hex8(preview.fg) },
    });

    Ok(json!({
        "wall": wall_json,
        "editor": editor,
        "picker": {
            "open": b.get_picker_open(),
            "label": b.get_picker_label().to_string(),
            "now": hex8(b.get_picker_now()),
            "start": hex8(b.get_picker_start()),
            "solid": hex8(b.get_picker_solid()),
        },
        "ask": {
            "open": b.get_ask_open(),
            "buttons": ASK_BUTTONS,
        },
        "saves": saves(ctx, &f),
    }))
}

/// `settings.colours`: the accent the window paints and the ink its primary buttons wear, as
/// `Theme` holds them (`#rrggbbaa`).
fn settings_colours(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let t = ctx.app.global::<Theme>();
    Ok(json!({
        "accent": hex8(t.get_accent()),
        "accent_solid": hex8(t.get_accent_solid()),
        "on_accent": hex8(t.get_on_accent()),
        "ground": hex8(t.get_ground()),
    }))
}

/// `settings.row { id }`: one row of the open page: its rectangle, whether it is inside the pane,
/// its kind, its value, a Reset link, and for a colour row its code box and swatch; for a select
/// its option names. `null` when the page has no such row.
fn settings_row(ctx: &ProbeCtx, arg: &Value) -> Result<Value, String> {
    let id = arg
        .get("id")
        .and_then(Value::as_str)
        .ok_or("settings.row needs an id")?;
    let b = ctx.app.global::<SettingsBridge>();
    let f = frame(ctx);
    for s in b.get_sections().iter() {
        for r in s.rows.iter() {
            if r.id.as_str() != id {
                continue;
            }
            let panel = f.panel(f64::from(s.panel_y), f64::from(s.panel_h));
            let row = f.row(panel, f64::from(r.y), f64::from(r.h));
            let kind = format!("{:?}", r.kind).to_lowercase();
            let mut out = json!({
                "id": id,
                "rect": row.json(),
                "visible": f.in_pane(row),
                "kind": kind,
                "value": r.value.to_string(),
                "on": r.on,
                "reset": r.reset,
                "save_dirty": r.save_dirty,
                "colour": hex8(r.colour),
                "code": r.code.to_string(),
                "options": r.options.iter().map(|o| o.name.to_string()).collect::<Vec<_>>(),
            });
            if kind == "colour" {
                out["code_rect"] = colour_code(row, r.code_wide).json();
                out["swatch"] = colour_swatch(row).json();
            }
            return Ok(out);
        }
    }
    Ok(Value::Null)
}
