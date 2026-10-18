//! Settings C probes (Task 35): the colour popover, the theme wall and its editor, the switch
//! question, the Save button, the window's paint and the working mark. Read-only: every answer is
//! read from the `SettingsBridge` global (which the controller writes), from the Slint theme or from
//! the tab rows, and nothing here sets a property. Rectangles are css pixels in window-client
//! coordinates, as `ui.rect` gives them. Nothing runs without --e2e, because `e2e_glue::install` is
//! the only caller.
//!
//! The Settings page is laid out by `flare_settings::layout` and drawn by `view.slint`; the swatch,
//! the slider, the wall card and the editor's wells sit at fixed offsets inside their rows and boxes,
//! and the small pure functions below say where, from the same numbers the `.slint` files use. The
//! scenarios click what these probes return, so a wrong offset shows as a click that does nothing.

use flare_settings::layout::{Metrics, is_narrow, rail_width};
use serde_json::{Value, json};
use slint::{ComponentHandle, Model};

use super::{ProbeCtx, ProbeRegistry};
use crate::{HelpBridge, SettingsBridge, Theme};

/// The colour popover's width, and its height with and without the alpha bar (`colour.slint`).
const POP_W: f64 = 226.0;
const POP_H_ALPHA: f64 = 254.0;
const POP_H_PLAIN: f64 = 230.0;
/// A colour field's swatch (`ColourField`): 36 by 24, 6 px right of the code box.
const SWATCH_W: f64 = 36.0;
const SWATCH_H: f64 = 24.0;
/// The row's right padding (`SetRowFrame`) and the slider's width (`RowControls`).
const ROW_PAD_RIGHT: f64 = 16.0;
const SLIDER_W: f64 = 218.0;
const SLIDER_H: f64 = 32.0;
/// The wall block's padding (`SetBlockFrame` with `pad`) and the wall's top offset in its row.
const WALL_PAD_X: f64 = 16.0;
const WALL_TOP: f64 = 14.0;
/// A wall card, and its pencil's inset (`ThemeCard`).
const CARD_W: f64 = 196.0;
const CARD_H: f64 = 124.0;
const PENCIL: f64 = 20.0;
const TOGGLE_W: f64 = 40.0;
const TOGGLE_H: f64 = 28.0;

/// The colour source file the accessible names are read from (the probe returns the literals the
/// window really carries, so a rename there shows in the scenario).
const COLOUR_SLINT: &str = include_str!("../../ui/settings/colour.slint");

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("picker.state", picker_state);
    r.add("picker.row", picker_row);
    r.add("picker.wall", picker_wall);
    r.add("picker.editor", picker_editor);
    r.add("picker.layers", picker_layers);
    r.add("picker.save", picker_save);
    r.add("glass.paint", glass_paint);
    r.add("agent.working", agent_working);
}

// ---- pure geometry -----------------------------------------------------------------------------------

/// A rectangle in css pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn json(self) -> Value {
        json!({ "x": self.x, "y": self.y, "w": self.w, "h": self.h })
    }

    fn right(self) -> f64 {
        self.x + self.w
    }

    fn bottom(self) -> f64 {
        self.y + self.h
    }
}

/// `Px.snap`: a length rounded to whole device pixels.
fn snap(l: f64, k: f64) -> f64 {
    (l * k).round() / k
}

/// `Px.hair()`: `max(1, floor(k))` device pixels.
fn hair(k: f64) -> f64 {
    k.floor().max(1.0) / k
}

/// Where the Settings frame is in the window and how far its pane is scrolled.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub left: f64,
    pub top: f64,
    pub w: f64,
    pub k: f64,
    pub scroll: f64,
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

    /// The panel's width: the column, capped, less both paddings.
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
}

/// A colour row's swatch: right-aligned in its row, 16 px in, centred vertically.
pub fn colour_swatch(row: Rect) -> Rect {
    Rect {
        x: row.right() - ROW_PAD_RIGHT - SWATCH_W,
        y: row.y + (row.h - SWATCH_H) / 2.0,
        w: SWATCH_W,
        h: SWATCH_H,
    }
}

/// A colour row's code box, left of the swatch.
pub fn colour_code(row: Rect, wide: bool) -> Rect {
    let sw = colour_swatch(row);
    let w = if wide { 172.0 } else { 76.0 };
    Rect {
        x: sw.x - 6.0 - w,
        y: sw.y + 1.0,
        w,
        h: 22.0,
    }
}

/// A slider row's slider: right-aligned, 16 px in, centred.
pub fn row_slider(row: Rect) -> Rect {
    Rect {
        x: row.right() - ROW_PAD_RIGHT - SLIDER_W,
        y: row.y + (row.h - SLIDER_H) / 2.0,
        w: SLIDER_W,
        h: SLIDER_H,
    }
}

/// The wall's own box in a wall block's row.
pub fn wall_box(row: Rect) -> Rect {
    Rect {
        x: row.x + WALL_PAD_X,
        y: row.y + WALL_TOP,
        w: row.w - 2.0 * WALL_PAD_X,
        h: 0.0,
    }
}

/// A wall card at wall-relative `(cx, cy)`, and its pencil.
pub fn wall_card(wall: Rect, cx: f64, cy: f64, k: f64) -> (Rect, Rect) {
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

/// The wall's show-all toggle, 8 px under the shown height.
pub fn wall_toggle(wall: Rect, shown_h: f64, k: f64) -> Rect {
    Rect {
        x: wall.x + snap((wall.w - TOGGLE_W) / 2.0, k),
        y: wall.y + snap(shown_h + 8.0, k),
        w: TOGGLE_W,
        h: TOGGLE_H,
    }
}

/// The popover's parts, from its top-left corner (`ColourPopover`).
pub fn popover_parts(x: f64, y: f64, alpha_on: bool) -> Value {
    let (pw, ph) = (POP_W, if alpha_on { POP_H_ALPHA } else { POP_H_PLAIN });
    let foot_y = if alpha_on { 213.0 } else { 189.0 };
    let part = |dx: f64, dy: f64, w: f64, h: f64| Rect {
        x: x + dx,
        y: y + dy,
        w,
        h,
    };
    json!({
        "box": part(0.0, 0.0, pw, ph).json(),
        "sv": part(13.0, 13.0, 200.0, 140.0).json(),
        "hue": part(13.0, 165.0, 200.0, 12.0).json(),
        "alpha": if alpha_on { part(13.0, 189.0, 200.0, 12.0).json() } else { Value::Null },
        "dropper": part(13.0, foot_y, 28.0, 28.0).json(),
        "format": part(159.0, foot_y, 54.0, 28.0).json(),
    })
}

/// The theme editor's boxes in a window `win_w` by `win_h`, with `n` wells (two to a row, the first
/// on its own); `any_wide` is true when a code field is in a long format. The wells' swatches, the
/// Save and Cancel buttons (their widths are the words', so the width here is a lower bound).
pub struct EditorGeo {
    pub box_: Rect,
    pub beside: bool,
    pub swatches: Vec<Rect>,
    pub save: Rect,
    pub cancel: Rect,
    pub card: Rect,
}

pub fn editor_geometry(win_w: f64, win_h: f64, k: f64, n: usize, any_wide: bool) -> EditorGeo {
    let field_w = if any_wide { 214.0 } else { 118.0 };
    let col_w = 86.0 + 8.0 + field_w;
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
    let title_h = 20.0;
    let inner_h = grid_h + if beside { 0.0 } else { 24.0 + card_h };
    let content_w = if beside {
        grid_w + 24.0 + card_w
    } else {
        grid_w.max(card_w)
    };
    let content_h = 20.0 + title_h + 12.0 + inner_h + 16.0 + 32.0 + 20.0;
    let box_w = snap(avail.min(content_w + 40.0), k);
    let box_h = snap(content_h.min(win_h * 0.85), k);
    let box_x = snap((win_w - box_w) / 2.0, k);
    let box_y = snap((win_h - box_h) / 2.0, k);
    let grid_x = box_x + 20.0;
    let grid_y = box_y + 20.0 + title_h + 12.0;
    let swatches = (0..n)
        .map(|j| {
            let col = (j % 2) as f64;
            let row = (j / 2) as f64;
            let cell_x = snap(20.0 + col * (col_w + 24.0), k);
            let cell_y = snap(20.0 + title_h + 12.0 + row * 30.0, k);
            Rect {
                x: box_x + cell_x + col_w - SWATCH_W,
                y: box_y + cell_y,
                w: SWATCH_W,
                h: SWATCH_H,
            }
        })
        .collect();
    let button_y = grid_y + inner_h + 16.0;
    EditorGeo {
        box_: Rect {
            x: box_x,
            y: box_y,
            w: box_w,
            h: box_h,
        },
        beside,
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

/// The text of `out property <string> <name>: "...";` in a Slint source, or None.
pub fn slint_string(src: &str, name: &str) -> Option<String> {
    let key = format!("property <string> {name}:");
    let at = src.find(&key)? + key.len();
    let rest = src[at..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// The word the swatch's accessible label starts with (`accessible-label: "Pick " + root.label;`).
pub fn swatch_prefix(src: &str) -> Option<String> {
    let key = "accessible-label: \"";
    let mut from = 0;
    while let Some(i) = src[from..].find(key) {
        let start = from + i + key.len();
        let end = start + src[start..].find('"')?;
        let tail = src[end + 1..].trim_start();
        if tail.starts_with("+ root.label") {
            return Some(src[start..end].to_string());
        }
        from = end;
    }
    None
}

/// The accessible names the popover and its swatch carry.
pub fn names_json() -> Value {
    json!({
        "sv": slint_string(COLOUR_SLINT, "sv-name"),
        "hue": slint_string(COLOUR_SLINT, "hue-name"),
        "alpha": slint_string(COLOUR_SLINT, "alpha-name"),
        "swatch_prefix": swatch_prefix(COLOUR_SLINT),
    })
}

// ---- reading the window ------------------------------------------------------------------------------

fn hex8(c: slint::Color) -> String {
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        c.red(),
        c.green(),
        c.blue(),
        c.alpha()
    )
}

fn frame_of(ctx: &ProbeCtx) -> Frame {
    let k = f64::from(ctx.app.window().scale_factor());
    Frame {
        left: ctx.app.get_grid_left() as f64,
        top: ctx.app.get_grid_top() as f64,
        w: ctx.app.get_grid_phys_width() as f64 / k,
        k,
        scroll: ctx.app.global::<SettingsBridge>().get_scroll_y() as f64,
    }
}

fn css_window(ctx: &ProbeCtx) -> (f64, f64) {
    let s = ctx.app.window().size();
    let k = f64::from(ctx.app.window().scale_factor());
    (s.width as f64 / k, s.height as f64 / k)
}

/// The pane's visible height in css px (the view reports it).
fn pane_h(ctx: &ProbeCtx) -> f64 {
    ctx.app.global::<SettingsBridge>().get_pane_h() as f64
}

/// Whether a window-space rectangle lies inside the pane on screen (so a click reaches it).
fn in_pane(ctx: &ProbeCtx, r: Rect) -> bool {
    let f = frame_of(ctx);
    let top = f.top;
    let bottom = f.top + pane_h(ctx);
    r.y >= top && r.bottom() <= bottom
}

/// `picker.state`: the colour popover as the bridge holds it.
fn picker_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let b = ctx.app.global::<SettingsBridge>();
    let alpha_on = b.get_picker_alpha_on();
    let open = b.get_picker_open();
    let (x, y) = (b.get_picker_x() as f64, b.get_picker_y() as f64);
    Ok(json!({
        "open": open,
        "label": b.get_picker_label().to_string(),
        "format": b.get_picker_format().to_string(),
        "alpha_on": alpha_on,
        "alpha_disabled": b.get_picker_alpha_disabled(),
        "dropper_on": b.get_picker_dropper_on(),
        "sv_now": b.get_picker_sv_now(),
        "hue_now": b.get_picker_hue_now(),
        "alpha_now": b.get_picker_alpha_now(),
        "sv_text": b.get_picker_sv_text().to_string(),
        "hue_text": b.get_picker_hue_text().to_string(),
        "alpha_text": b.get_picker_alpha_text().to_string(),
        "solid": hex8(b.get_picker_solid()),
        "start": hex8(b.get_picker_start()),
        "now": hex8(b.get_picker_now()),
        "names": names_json(),
        "parts": if open { popover_parts(x, y, alpha_on) } else { Value::Null },
        "editor_open": b.get_editor_open(),
        "ask_open": b.get_ask_open(),
    }))
}

/// `picker.row { id }`: one row of the open page. Its kind, its control's value, its rectangle and
/// the control's rectangle in window css px, and whether it lies inside the pane on screen.
fn picker_row(ctx: &ProbeCtx, arg: &Value) -> Result<Value, String> {
    let id = arg
        .get("id")
        .and_then(Value::as_str)
        .ok_or("picker.row needs an id")?;
    let b = ctx.app.global::<SettingsBridge>();
    let f = frame_of(ctx);
    for s in b.get_sections().iter() {
        for r in s.rows.iter() {
            if r.id.as_str() != id {
                continue;
            }
            let panel = f.panel(s.panel_y as f64, s.panel_h as f64);
            let row = f.row(panel, r.y as f64, r.h as f64);
            let kind = format!("{:?}", r.kind).to_lowercase();
            let control = match kind.as_str() {
                "colour" => Some(colour_swatch(row)),
                "slider" => Some(row_slider(row)),
                _ => None,
            };
            let code_box = (kind == "colour").then(|| colour_code(row, r.code_wide));
            return Ok(json!({
                "id": id,
                "kind": kind,
                "label": r.label.to_string(),
                "sub": r.sub.to_string(),
                "on": r.on,
                "off": r.off,
                "section": s.id.to_string(),
                "value": r.value.to_string(),
                "num": r.num,
                "num_max": r.num_max,
                "num_text": r.num_text.to_string(),
                "code": r.code.to_string(),
                "code_wide": r.code_wide,
                "colour": hex8(r.colour),
                "reset": r.reset,
                "rect": row.json(),
                "control": control.map(Rect::json),
                "code_box": code_box.map(Rect::json),
                "visible": in_pane(ctx, row),
                "scroll_y": f.scroll,
            }));
        }
    }
    Err(format!("picker.row: no row {id} on the page in front"))
}

/// `picker.save`: the first section heading that carries a Save button: its id, whether it is lit,
/// its title and a point inside the button (the heading's right edge, in window css px).
fn picker_save(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let b = ctx.app.global::<SettingsBridge>();
    let f = frame_of(ctx);
    for s in b.get_sections().iter() {
        if !s.action_save {
            continue;
        }
        let panel = f.panel(s.panel_y as f64, s.panel_h as f64);
        let y = f.top + s.title_y as f64 - f.scroll;
        // The button is right-aligned to the heading, 2 px in; a point 14 px left of that is inside
        // even for the shortest word.
        let point = json!({ "x": panel.right() - 2.0 - 14.0, "y": y + 10.0 });
        let area = Rect {
            x: panel.x,
            y,
            w: panel.w,
            h: 20.0,
        };
        return Ok(json!({
            "section": s.id.to_string(),
            "dirty": s.action_dirty,
            "title": s.action_title.to_string(),
            "point": point,
            "visible": in_pane(ctx, area),
        }));
    }
    Err("picker.save: no section on this page has a Save".into())
}

/// `picker.wall`: the theme wall: whether it is open, each card (id, name, on, the pencil showing,
/// its rectangle and its pencil's, its ground and ink), the toggle, and the clip's height.
fn picker_wall(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let b = ctx.app.global::<SettingsBridge>();
    let f = frame_of(ctx);
    let mut wall_row = None;
    for s in b.get_sections().iter() {
        for r in s.rows.iter() {
            if r.id.as_str() == "wall" || format!("{:?}", r.block).to_lowercase() == "wall" {
                let panel = f.panel(s.panel_y as f64, s.panel_h as f64);
                wall_row = Some(f.row(panel, r.y as f64, r.h as f64));
            }
        }
    }
    let row = wall_row.ok_or("picker.wall: the page in front has no theme wall")?;
    let wall = wall_box(row);
    let expanded = b.get_wall_expanded();
    let shown_h = if expanded {
        b.get_wall_full_h() as f64
    } else {
        b.get_wall_clip_h() as f64
    };
    let cards: Vec<Value> = b
        .get_wall_cards()
        .iter()
        .map(|c| {
            let (card, pencil) = wall_card(wall, c.x as f64, c.y as f64, f.k);
            // A card is hit only while the clip shows it, and while it is inside the pane.
            let shown = c.y as f64 + CARD_H <= shown_h + 8.0;
            json!({
                "id": c.id.to_string(),
                "name": c.name.to_string(),
                "on": c.on,
                "edit": c.edit,
                "rect": card.json(),
                "pencil": pencil.json(),
                "shown": shown,
                "visible": shown && in_pane(ctx, card),
                "bg": hex8(c.bg),
                "fg": hex8(c.fg),
            })
        })
        .collect();
    let toggle = wall_toggle(wall, shown_h, f.k);
    Ok(json!({
        "expanded": expanded,
        "shown_h": shown_h,
        "full_h": b.get_wall_full_h(),
        "cards": cards,
        "toggle": toggle.json(),
        "toggle_visible": in_pane(ctx, toggle),
        "toggle_title": b.get_wall_toggle_title().to_string(),
        "rect": wall.json(),
    }))
}

/// `picker.editor`: the theme editor: open or not, each well (key, label, the code shown, its colour,
/// wide, open, the swatch's rectangle), the preview card's ground and ink, and the buttons.
fn picker_editor(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let b = ctx.app.global::<SettingsBridge>();
    let (ww, wh) = css_window(ctx);
    let k = f64::from(ctx.app.window().scale_factor());
    let wells: Vec<_> = b.get_editor_wells().iter().collect();
    let any_wide = wells.iter().any(|w| w.wide);
    let geo = editor_geometry(ww, wh, k, wells.len(), any_wide);
    let wells_json: Vec<Value> = wells
        .iter()
        .zip(&geo.swatches)
        .map(|(w, sw)| {
            json!({
                "key": w.key.to_string(),
                "label": w.label.to_string(),
                "text": w.text.to_string(),
                "colour": hex8(w.colour),
                "wide": w.wide,
                "open": w.open,
                "swatch": sw.json(),
            })
        })
        .collect();
    let p = b.get_editor_preview();
    Ok(json!({
        "open": b.get_editor_open(),
        "wells": wells_json,
        "preview": { "bg": hex8(p.bg), "fg": hex8(p.fg), "id": p.id.to_string() },
        "box": geo.box_.json(),
        "beside": geo.beside,
        "save": geo.save.json(),
        "cancel": geo.cancel.json(),
        "card": geo.card.json(),
    }))
}

/// `picker.layers`: what is on top of the page: Command help, the update window and its chip, the
/// editor, the question and the popover.
fn picker_layers(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let a = ctx.app;
    let b = a.global::<SettingsBridge>();
    Ok(json!({
        "help_open": a.global::<HelpBridge>().get_open(),
        "update_open": a.get_update_open(),
        "update_chip": {
            "shown": a.get_update_chip_shown(),
            "x": a.get_update_chip_x(),
            "y": a.get_update_chip_y(),
            "w": a.get_update_chip_w(),
            "h": a.get_update_chip_h(),
        },
        "editor_open": b.get_editor_open(),
        "ask_open": b.get_ask_open(),
        "picker_open": b.get_picker_open(),
    }))
}

/// `glass.paint`: the window's grounds as the Slint theme holds them now, with their alpha: the
/// page (`--p-bg`), the title row, the tab strip, the settings rail, and the solid ground.
fn glass_paint(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let t = ctx.app.global::<Theme>();
    let alpha = |c: slint::Color| f64::from(c.alpha()) / 255.0;
    let (g, ti, ta, si) = (t.get_ground(), t.get_title(), t.get_tabs(), t.get_side());
    Ok(json!({
        "ground": hex8(g),
        "title": hex8(ti),
        "tabs": hex8(ta),
        "side": hex8(si),
        "solid": hex8(t.get_ground_solid()),
        "clear": hex8(t.get_window_clear()),
        "text": hex8(t.get_text()),
        "alpha": { "ground": alpha(g), "title": alpha(ti), "tabs": alpha(ta), "side": alpha(si) },
    }))
}

/// `agent.working`: the colour (with alpha) the first working tab's mark wears, or null.
fn agent_working(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    for row in ctx.app.get_tab_rows().iter() {
        if row.mark_state == 1 {
            return Ok(json!({
                "tab": row.id.to_string(),
                "colour": hex8(row.mark_color),
                "alpha": row.mark_color.alpha(),
                "kind": row.mark_kind,
            }));
        }
    }
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALES: [f64; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];

    fn frame(w: f64, k: f64) -> Frame {
        Frame {
            left: 0.0,
            top: 36.0,
            w,
            k,
            scroll: 0.0,
        }
    }

    #[test]
    fn the_popover_names_are_the_slint_literals() {
        let n = names_json();
        assert_eq!(n["sv"], json!("Saturation and brightness"));
        assert_eq!(n["hue"], json!("Hue"));
        assert_eq!(n["alpha"], json!("Alpha"));
        assert_eq!(n["swatch_prefix"], json!("Pick "));
    }

    #[test]
    fn a_missing_slint_property_reads_as_none() {
        assert_eq!(slint_string("out property <string> a: \"x\";", "b"), None);
        assert_eq!(
            slint_string("out property <string> a: \"x\";", "a"),
            Some("x".to_string())
        );
        assert_eq!(swatch_prefix("accessible-label: \"Nope\";"), None);
    }

    #[test]
    fn a_colour_swatch_sits_at_the_row_right_inside_the_row_at_every_scale() {
        for k in SCALES {
            for w in [900.0, 1600.0] {
                let f = frame(w, k);
                let panel = f.panel(100.0, 400.0);
                let row = f.row(panel, 58.0, 58.0);
                let sw = colour_swatch(row);
                assert!(sw.x > row.x && sw.right() < row.right(), "{k} {w}");
                assert!(sw.y >= row.y && sw.bottom() <= row.bottom(), "{k} {w}");
                // The code box is left of the swatch, wider when long.
                let narrow = colour_code(row, false);
                let wide = colour_code(row, true);
                assert!(narrow.right() < sw.x && wide.right() < sw.x);
                assert!(wide.w > narrow.w);
            }
        }
    }

    #[test]
    fn the_pane_is_capped_at_the_column_and_the_narrow_frame_uses_the_icon_rail() {
        let wide = frame(1600.0, 1.0);
        assert_eq!(wide.pane_x(), 244.0);
        assert_eq!(wide.content_w(), 960.0 - 88.0);
        let narrow = frame(700.0, 1.0);
        assert_eq!(narrow.pane_x(), 56.0);
        assert_eq!(narrow.pad(), 20.0);
        assert_eq!(narrow.content_w(), 700.0 - 56.0 - 40.0);
    }

    #[test]
    fn scrolling_moves_a_row_up_by_the_scroll() {
        let mut f = frame(1600.0, 2.0);
        let a = f.row(f.panel(100.0, 400.0), 58.0, 58.0);
        f.scroll = 40.0;
        let b = f.row(f.panel(100.0, 400.0), 58.0, 58.0);
        assert_eq!(a.y - b.y, 40.0);
        assert_eq!(a.x, b.x);
    }

    #[test]
    fn the_slider_is_218_wide_and_ends_16_px_in() {
        let f = frame(1600.0, 1.0);
        let row = f.row(f.panel(100.0, 400.0), 0.0, 58.0);
        let s = row_slider(row);
        assert_eq!(s.w, 218.0);
        assert_eq!(row.right() - s.right(), 16.0);
        assert!(s.y >= row.y && s.bottom() <= row.bottom());
    }

    #[test]
    fn a_card_and_its_pencil_lie_inside_the_card_at_every_scale() {
        for k in SCALES {
            let f = frame(1600.0, k);
            let row = f.row(f.panel(100.0, 900.0), 0.0, 400.0);
            let wall = wall_box(row);
            let (card, pencil) = wall_card(wall, 208.0, 138.0, k);
            assert!(pencil.x >= card.x && pencil.right() <= card.right(), "{k}");
            assert!(
                pencil.y >= card.y && pencil.bottom() <= card.bottom(),
                "{k}"
            );
            assert_eq!(card.x, wall.x + 208.0);
            let t = wall_toggle(wall, 268.0, k);
            assert!(t.y > wall.y + 268.0, "the toggle is under the clip at {k}");
            assert!(
                (t.x + t.w / 2.0 - (wall.x + wall.w / 2.0)).abs() <= 1.0,
                "{k}"
            );
        }
    }

    #[test]
    fn the_editor_has_twenty_swatches_in_two_columns_and_the_buttons_under_them() {
        for k in SCALES {
            let g = editor_geometry(1600.0, 1000.0, k, 20, false);
            assert!(
                g.beside,
                "the card sits beside the grid in a wide window at {k}"
            );
            assert_eq!(g.swatches.len(), 20);
            // Two columns: even wells left, odd wells right; ten rows, 30 px apart.
            assert!(g.swatches[1].x > g.swatches[0].x + 100.0);
            assert!((g.swatches[2].y - g.swatches[0].y - 30.0).abs() <= 1.0);
            assert!(
                g.swatches
                    .iter()
                    .all(|s| s.right() <= g.box_.right() && s.y >= g.box_.y)
            );
            // The buttons are under the last row.
            let last = g.swatches[19];
            assert!(g.save.y > last.bottom(), "{k}");
            assert!(g.cancel.x > g.save.x);
            assert!(g.save.bottom() <= g.box_.bottom() + 1.0, "{k}");
        }
    }

    #[test]
    fn the_card_goes_under_the_grid_in_a_narrow_window_and_a_long_format_widens_the_columns() {
        let narrow = editor_geometry(700.0, 1000.0, 1.0, 20, false);
        assert!(!narrow.beside);
        let short = editor_geometry(1600.0, 1000.0, 1.0, 20, false);
        let long = editor_geometry(1600.0, 1000.0, 1.0, 20, true);
        assert!(long.box_.w > short.box_.w);
        assert!(
            long.swatches[1].x - long.swatches[0].x > short.swatches[1].x - short.swatches[0].x
        );
    }

    #[test]
    fn the_popover_parts_follow_the_alpha_bar() {
        let with = popover_parts(100.0, 50.0, true);
        let without = popover_parts(100.0, 50.0, false);
        assert_eq!(with["box"]["h"], json!(254.0));
        assert_eq!(without["box"]["h"], json!(230.0));
        assert!(without["alpha"].is_null());
        assert_eq!(with["alpha"]["y"], json!(50.0 + 189.0));
        assert_eq!(with["format"]["y"], json!(50.0 + 213.0));
        assert_eq!(without["format"]["y"], json!(50.0 + 189.0));
        // The popover's width is the constant the controller places by.
        assert_eq!(with["box"]["w"], json!(226.0));
    }
}
