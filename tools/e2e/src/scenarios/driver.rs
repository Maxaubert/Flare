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
