//! The theme-pick driver the scenarios share (M10 D11): Settings from the cog, the Appearance
//! page, the wall opened, the card wheeled into view and clicked, by pointer on the real wall (a
//! pick is a write, so there is no write probe for it).

use flare_e2e_proto::{PointerAction, Request};
use serde_json::{json, Value};

use crate::ctx::{App, Ctx};

/// A rectangle in css px: x, y, w, h.
type Rect4 = (f64, f64, f64, f64);

fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

fn rect_of(v: &Value) -> Option<Rect4> {
    Some((
        v["x"].as_f64()?,
        v["y"].as_f64()?,
        v["w"].as_f64()?,
        v["h"].as_f64()?,
    ))
}

fn centre(r: Rect4) -> (f32, f32) {
    ((r.0 + r.2 / 2.0) as f32, (r.1 + r.3 / 2.0) as f32)
}

fn ui_rect(app: &mut App, name: &str) -> Option<Rect4> {
    rect_of(&probe(app, "ui.rect", json!({ "name": name })))
}

/// Whether Settings is the tab in front.
pub fn settings_front(app: &mut App) -> bool {
    probe(app, "settings.state", Value::Null)["front"] == json!(true)
}

/// Waits up to `ms` for `cond` without logging a failure when it does not come: a question that
/// may or may not appear. True when it did.
pub fn wait_quiet(ms: u64, cond: &mut dyn FnMut() -> bool) -> bool {
    crate::until::wait_on(&mut crate::until::SystemClock::new(), ms, cond)
}

/// Whether the switch question of the theme wall is open.
fn ask_open(app: &mut App) -> bool {
    probe(app, "settings.theme", Value::Null)["ask"]["open"] == json!(true)
}

/// One card of the theme wall: its id, rectangle, whether it is the theme in force, and whether it
/// is in view.
#[derive(Debug, Clone)]
pub struct Card {
    pub id: String,
    pub rect: Rect4,
    pub on: bool,
    pub visible: bool,
}

/// The cards of `picker.wall` in order (empty when the wall is not drawn).
pub fn wall_cards(wall: &Value) -> Vec<Card> {
    wall["cards"]
        .as_array()
        .map(|cards| {
            cards
                .iter()
                .filter_map(|c| {
                    Some(Card {
                        id: c["id"].as_str()?.to_string(),
                        rect: rect_of(&c["rect"])?,
                        on: c["on"] == json!(true),
                        visible: c["visible"] == json!(true),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Picks the theme `id` on the Settings wall. A pick over unsaved changes asks first, and Discard
/// is taken (Tab, Tab, Enter), as the other scenarios do. True when `id` is in force after.
pub fn pick_theme(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    if !settings_front(app) {
        if let Some(r) = ui_rect(app, "cog") {
            let (x, y) = centre(r);
            let _ = app.session.click(x, y);
        }
        if !ctx.until("Settings in front", 8_000, &mut || settings_front(app)) {
            return false;
        }
    }
    if probe(app, "settings.rows", json!({}))["page"] != json!("appearance") {
        let rail = probe(app, "settings.rail", json!({ "id": "appearance" }));
        if let Some(r) = rect_of(&rail) {
            let (x, y) = centre(r);
            let _ = app.session.click(x, y);
        }
        let there = ctx.until("the Appearance page", 6_000, &mut || {
            probe(app, "settings.rows", json!({}))["page"] == json!("appearance")
        });
        if !there {
            return false;
        }
    }
    let wall = probe(app, "picker.wall", Value::Null);
    if wall["expanded"] == json!(false) {
        // The toggle sits under the clipped cards: wheel it into the pane before it is clicked (a
        // later card, such as Paper, needs the wall open, and the toggle may be below the fold).
        ctx.until("the wall's toggle in view", 8_000, &mut || {
            let wall = probe(app, "picker.wall", Value::Null);
            if wall["toggle_visible"] == json!(true) {
                return true;
            }
            let grid = ui_rect(app, "grid");
            let mid = grid.map_or(300.0, |g| g.1 + g.3 / 2.0);
            let (x, y) = grid.map_or((400.0, 300.0), centre);
            let below = rect_of(&wall["toggle"]).is_none_or(|t| t.1 > mid);
            let _ = app.session.call(&Request::Pointer {
                x,
                y,
                action: PointerAction::Wheel {
                    dx: 0.0,
                    dy: if below { -160.0 } else { 160.0 },
                },
            });
            false
        });
        let wall = probe(app, "picker.wall", Value::Null);
        if let Some(r) = rect_of(&wall["toggle"]) {
            let (x, y) = centre(r);
            let _ = app.session.click(x, y);
        }
        ctx.until("the wall open", 4_000, &mut || {
            probe(app, "picker.wall", Value::Null)["expanded"] == json!(true)
        });
    }
    let in_view = ctx.until("the card in view", 12_000, &mut || {
        let wall = probe(app, "picker.wall", Value::Null);
        let Some(card) = wall_cards(&wall).into_iter().find(|c| c.id == id) else {
            return false;
        };
        if card.visible {
            return true;
        }
        let grid = ui_rect(app, "grid");
        let mid = grid.map_or(300.0, |g| g.1 + g.3 / 2.0);
        let (x, y) = grid.map_or((400.0, 300.0), centre);
        let dy = if card.rect.1 > mid { -160.0 } else { 160.0 };
        let _ = app.session.call(&Request::Pointer {
            x,
            y,
            action: PointerAction::Wheel { dx: 0.0, dy },
        });
        false
    });
    if !in_view {
        return false;
    }
    let wall = probe(app, "picker.wall", Value::Null);
    let Some(card) = wall_cards(&wall).into_iter().find(|c| c.id == id) else {
        return false;
    };
    let (x, y) = centre(card.rect);
    let _ = app.session.click(x, y);
    if wait_quiet(2_000, &mut || ask_open(app)) {
        let _ = app.session.key("Tab");
        let _ = app.session.key("Tab");
        let _ = app.session.key("Enter");
        wait_quiet(3_000, &mut || !ask_open(app));
    }
    ctx.until("the card on", 4_000, &mut || {
        wall_cards(&probe(app, "picker.wall", Value::Null))
            .iter()
            .any(|c| c.id == id && c.on)
    })
}

// ---------------------------------------------------------------- the Settings rows driver
//
// PT's `gotoPref` and `setPref`: Settings open, the page that holds a row shown, the row wheeled
// into view, a pill row picked by the pointer. Shared by the strip, chrome and lifecycle ports.

/// `settings.row` for `id` (`Null` when the open page draws no such row).
pub fn row(app: &mut App, id: &str) -> Value {
    probe(app, "settings.row", json!({ "id": id }))
}

/// The catalogue of every row (`settings.rows` with `catalogue`), in draw order.
pub fn catalogue(app: &mut App) -> Vec<Value> {
    probe(app, "settings.rows", json!({ "catalogue": true }))["rows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The page that holds row `id`, from the catalogue.
pub fn page_of(app: &mut App, id: &str) -> Option<String> {
    catalogue(app)
        .iter()
        .find(|r| r["id"].as_str() == Some(id))
        .and_then(|r| r["page"].as_str().map(str::to_string))
}

fn page_now(app: &mut App) -> Option<String> {
    probe(app, "settings.rows", json!({}))["page"]
        .as_str()
        .map(str::to_string)
}

/// Opens Settings from the cog and waits until it is in front.
pub fn open_settings(ctx: &mut Ctx, app: &mut App) -> bool {
    if settings_front(app) {
        return true;
    }
    match ui_rect(app, "cog") {
        Some(r) => {
            let (x, y) = centre(r);
            let _ = app.session.click(x, y);
        }
        None => return false,
    }
    ctx.until("Settings is in front", 8_000, &mut || settings_front(app))
}

/// Chooses a rail page by its rectangle and waits until its rows answer for it.
fn goto_page(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    if let Some(r) = rect_of(&probe(app, "settings.rail", json!({ "id": id }))) {
        let (x, y) = centre(r);
        let _ = app.session.click(x, y);
    }
    ctx.until(&format!("the {id} page is drawn"), 6_000, &mut || {
        page_now(app).as_deref() == Some(id)
    })
}

/// Wheels the pane until the row's box is inside the pane.
fn reveal(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    let (gx, gy, gw, gh) = ui_rect(app, "grid").unwrap_or((0.0, 36.0, 800.0, 600.0));
    let at = ((gx + gw / 2.0) as f32, (gy + gh / 2.0) as f32);
    ctx.until(&format!("row {id} to be in view"), 12_000, &mut || {
        let r = row(app, id);
        if r.is_null() {
            return false;
        }
        if r["visible"] == json!(true) {
            return true;
        }
        let y = r["rect"]["y"].as_f64().unwrap_or(0.0);
        let dy = if y > gy + gh / 2.0 { -160.0 } else { 160.0 };
        let _ = app.session.call(&Request::Pointer {
            x: at.0,
            y: at.1,
            action: PointerAction::Wheel { dx: 0.0, dy },
        });
        false
    })
}

/// PT's `gotoPref`: Settings open, the page that holds the row shown, the row in view and still.
pub fn goto_row(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    let Some(page) = page_of(app, id) else {
        return false;
    };
    if !open_settings(ctx, app) {
        return false;
    }
    if page_now(app).as_deref() != Some(page.as_str()) && !goto_page(ctx, app, &page) {
        return false;
    }
    if !reveal(ctx, app, id) {
        return false;
    }
    // The pane may still be gliding after the wheel: the row is clicked once it holds still.
    let mut last = Value::Null;
    ctx.until(&format!("row {id} to hold still"), 4_000, &mut || {
        let now = row(app, id)["rect"].clone();
        let still = now == last && !now.is_null();
        last = now;
        still
    })
}

/// True when the row's value is one of `want` (compared in lower case).
pub fn row_is(app: &mut App, id: &str, want: &[&str]) -> bool {
    let v = row(app, id)["value"].as_str().unwrap_or("").to_lowercase();
    want.iter().any(|w| v == *w)
}

/// PT's `setPref`: picks a segment of a pill row by clicking along the row from its right end until
/// the row says the wanted value. A pill is right-aligned, 16 px in from the row's edge.
pub fn pick_seg(ctx: &mut Ctx, app: &mut App, id: &str, want: &[&str]) -> bool {
    if !goto_row(ctx, app, id) {
        return false;
    }
    if row_is(app, id, want) {
        return true;
    }
    for step in 0..30 {
        let r = row(app, id);
        let Some((x, y, w, h)) = rect_of(&r["rect"]) else {
            return false;
        };
        let cx = x + w - 28.0 - f64::from(step) * 10.0;
        if cx < x + w / 2.0 {
            break;
        }
        let _ = app.session.click(cx as f32, (y + h / 2.0) as f32);
        for _ in 0..4 {
            if row_is(app, id, want) {
                return true;
            }
            ctx.settle(25);
        }
    }
    row_is(app, id, want)
}

/// Types a colour into a colour row's code field: the field is clicked, its text cleared, the new
/// text typed and Enter pressed (the row is brought into view first).
pub fn fill_row_colour(ctx: &mut Ctx, app: &mut App, id: &str, text: &str) -> bool {
    if !goto_row(ctx, app, id) {
        return false;
    }
    let Some(r) = rect_of(&row(app, id)["code_rect"]) else {
        return false;
    };
    let (x, y) = centre(r);
    let _ = app.session.click(x, y);
    let _ = app.session.key("End");
    for _ in 0..12 {
        let _ = app.session.key("Backspace");
    }
    let _ = app.session.type_text(text);
    let _ = app.session.key("Enter");
    true
}

/// Whether the Save changes of the page in front is lit: a section heading's (Mark colours) or the
/// Appearance page's own row (`picker.save` answers for either).
pub fn save_lit(app: &mut App) -> bool {
    probe(app, "picker.save", Value::Null)["dirty"] == json!(true)
}

/// Whether nothing on the open page runs past the pane's sides: every row's box lies inside the
/// frame (`ui.rect` grid), which is what "nothing scrolls sideways" means for a pane that never
/// scrolls that way. `None` when the page or the frame cannot be read.
pub fn nothing_sideways(app: &mut App) -> Option<bool> {
    let (gx, _, gw, _) = ui_rect(app, "grid")?;
    let rows = probe(app, "settings.rows", json!({}))["rows"]
        .as_array()?
        .clone();
    if rows.is_empty() {
        return None;
    }
    Some(rows.iter().all(|r| {
        let id = r["id"].as_str().unwrap_or("");
        rect_of(&row(app, id)["rect"])
            .is_none_or(|(x, _, w, _)| x >= gx - 0.5 && x + w <= gx + gw + 0.5)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wall_cards_read_their_id_rect_and_flags() {
        let wall = json!({ "cards": [
            { "id": "fawn", "rect": {"x": 1.0, "y": 2.0, "w": 3.0, "h": 4.0}, "on": true, "visible": false },
            { "id": "pt-default", "on": false, "visible": true }
        ]});
        let cards = wall_cards(&wall);
        assert_eq!(cards.len(), 1, "a card with no rectangle is not a card");
        assert_eq!(cards[0].id, "fawn");
        assert_eq!(cards[0].rect, (1.0, 2.0, 3.0, 4.0));
        assert!(cards[0].on && !cards[0].visible);
        assert!(wall_cards(&Value::Null).is_empty());
    }
}
