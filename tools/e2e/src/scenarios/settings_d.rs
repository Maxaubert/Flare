//! Settings D scenarios (Task 36): themeCards, edges, accent. Each follows its Prism Terminal lines
//! (PT: run.mjs 6580, 6683, 6864); a body says where Flare differs.
//!
//! Flare answers through `settings.row` and `picker.wall` (rectangles and values of the rows and the
//! wall cards), `theme.tokens` and `window.edge` (e2e_probes/settings_d.rs: the chrome tokens the
//! window paints, and the border the controller computes), `ui.rect` and `app.state` (the strip and
//! the tabs), `registry.writes`, and the stored settings in `<profile>\settings.json` (PT reads
//! localStorage). The PT checks that read the DOM's computed colours read the TOKEN the window paints
//! with instead, since Slint has no CSS: a token that moved while the paint did not would still
//! differ from the spec in the same way the DOM check would catch.
//!
//! What cannot be done as PT does, and how each is handled:
//! - A segmented pill has no rectangle the probes read (the settings_b rule), so its picks are not
//!   clicked: each edge word is written to settings.json and the app is launched again, and the
//!   check reads what the window then paints. The row's `value` (the pressed word) is read as PT's
//!   `aria-pressed`.
//! - The DWM border is never applied under --e2e (`material_ctl`), so "main was told" reads
//!   `window.edge`, the border the controller computes for the same ground and word.
//! - The See-through slider is GPU only (settings_c's SOFTWARE_SKIP), so the see-through halves of
//!   `accent` are skipped on the software renderer with that reason, never passed.
//! - No check here reads a painted pixel of a segment or a button, so none is claimed.

use std::collections::BTreeMap;
use std::path::Path;

use flare_e2e_proto::{PointerAction, Request};
use serde_json::{json, Value};

use crate::ctx::{App, Ctx, Launch};

/// A css rectangle: x, y, width, height.
type R = (f64, f64, f64, f64);

/// The reason the See-through halves of `accent` are skipped on the software renderer.
const SOFTWARE_SKIP: &str = "the See-through slider is GPU only (the software renderer disables it, settings_c's SOFTWARE_SKIP); this runs on the GPU renderer";
/// The tolerance for an alpha, the same one PT's `near` uses.
const NEAR: f64 = 0.004;
/// PT's black and white ink, as the knob rule compares them.
const BLACK: [f64; 3] = [11.0, 11.0, 15.0];
const WHITE: [f64; 3] = [255.0, 255.0, 255.0];

/// Polls `f` until it holds or `ms` passes, on the clock `ctx.until` uses. Unlike `ctx.until` it records
/// nothing: the caller records the one check the wait stands for, so a timeout is one FAIL, not two.
fn wait_for(ms: u64, f: &mut dyn FnMut() -> bool) -> bool {
    crate::until::wait_on(&mut crate::until::SystemClock::new(), ms, f)
}

// ---- small helpers -----------------------------------------------------------------------------------

fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

fn rect_of(v: &Value) -> Option<R> {
    Some((
        v["x"].as_f64()?,
        v["y"].as_f64()?,
        v["w"].as_f64()?,
        v["h"].as_f64()?,
    ))
}

fn centre(r: R) -> (f32, f32) {
    ((r.0 + r.2 / 2.0) as f32, (r.1 + r.3 / 2.0) as f32)
}

fn click_at(app: &mut App, x: f64, y: f64) -> bool {
    app.session.click(x as f32, y as f32).is_ok()
}

fn click_rect(app: &mut App, v: &Value) -> bool {
    match rect_of(v) {
        Some(r) => {
            let (x, y) = centre(r);
            app.session.click(x, y).is_ok()
        }
        None => false,
    }
}

fn ui_rect(app: &mut App, name: &str) -> Value {
    probe(app, "ui.rect", json!({ "name": name }))
}

fn tabs(app: &mut App) -> Vec<Value> {
    probe(app, "app.state", Value::Null)["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn settings_front(app: &mut App) -> bool {
    probe(app, "settings.state", Value::Null)["front"] == json!(true)
}

fn open_settings(app: &mut App) -> bool {
    let cog = ui_rect(app, "cog");
    if !click_rect(app, &cog) {
        return false;
    }
    wait_for(8_000, &mut || settings_front(app))
}

fn page_now(app: &mut App) -> Option<String> {
    probe(app, "settings.rows", json!({}))["page"]
        .as_str()
        .map(str::to_string)
}

fn goto_page(app: &mut App, id: &str) -> bool {
    let r = probe(app, "settings.rail", json!({ "id": id }));
    if let Some(r) = rect_of(&r) {
        let (x, y) = centre(r);
        let _ = app.session.click(x, y);
    }
    wait_for(6_000, &mut || page_now(app).as_deref() == Some(id))
}

/// The page a row lives on, from the catalogue (`settings.rows` with `catalogue`).
fn page_of(app: &mut App, id: &str) -> Option<String> {
    let cat = probe(app, "settings.rows", json!({ "catalogue": true }));
    cat["rows"]
        .as_array()?
        .iter()
        .find(|r| r["id"].as_str() == Some(id))
        .and_then(|r| r["page"].as_str().map(str::to_string))
}

/// One row of the page in front (`settings.row`), `Null` when the page has no such row.
fn row(app: &mut App, id: &str) -> Value {
    probe(app, "settings.row", json!({ "id": id }))
}

/// The row ids of the page in front, in draw order.
fn page_rows(app: &mut App) -> Vec<String> {
    probe(app, "settings.rows", json!({}))["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn pane_point(app: &mut App) -> (f32, f32) {
    let g = ui_rect(app, "grid");
    let (x, y, w, h) = rect_of(&g).unwrap_or((0.0, 36.0, 1000.0, 600.0));
    ((x + w / 2.0) as f32, (y + h / 2.0) as f32)
}

/// Wheels the pane until the row's box is inside the pane; true when it is.
fn reveal(app: &mut App, id: &str) -> bool {
    let at = pane_point(app);
    wait_for(12_000, &mut || {
        let r = row(app, id);
        if r.is_null() {
            return false;
        }
        if r["visible"] == json!(true) {
            return true;
        }
        let y = r["rect"]["y"].as_f64().unwrap_or(0.0);
        let g = ui_rect(app, "grid");
        let mid = rect_of(&g)
            .map(|(_, gy, _, gh)| gy + gh / 2.0)
            .unwrap_or(300.0);
        let dy = if y > mid { -160.0 } else { 160.0 };
        let _ = app.session.call(&Request::Pointer {
            x: at.0,
            y: at.1,
            action: PointerAction::Wheel { dx: 0.0, dy },
        });
        false
    })
}

/// Shows the page a row lives on and scrolls the row into view. False when either fails.
fn goto_row(app: &mut App, id: &str) -> bool {
    let Some(page) = page_of(app, id) else {
        return false;
    };
    if page_now(app).as_deref() != Some(page.as_str()) && !goto_page(app, &page) {
        return false;
    }
    reveal(app, id)
}

/// Clicks a text field at a css point, selects its text, types `text` and presses Enter.
fn fill_at(app: &mut App, x: f64, y: f64, text: &str) {
    let _ = click_at(app, x, y);
    let _ = app.session.key("End");
    for _ in 0..12 {
        let _ = app.session.key("Backspace");
    }
    let _ = app.session.type_text(text);
    let _ = app.session.key("Enter");
}

/// Types a colour into a colour row's code field; false when the row is not there.
fn fill_row_colour(app: &mut App, id: &str, text: &str) -> bool {
    if !goto_row(app, id) {
        return false;
    }
    match rect_of(&row(app, id)["code_rect"]) {
        Some((x, y, w, h)) => {
            fill_at(app, x + w / 2.0, y + h / 2.0, text);
            true
        }
        None => false,
    }
}

/// Presses a colour row's Reset link: it sits 10 px left of the code box (settings_c's rule).
fn press_reset(app: &mut App, id: &str) -> bool {
    match rect_of(&row(app, id)["code_rect"]) {
        Some(b) => click_at(app, b.0 - 10.0 - 14.0, b.1 + b.3 / 2.0),
        None => false,
    }
}

/// Holds the scenario still for `ms`, in 50 ms "no change" steps (the most `settle` allows).
fn hold(ctx: &mut Ctx, ms: u64) {
    for _ in 0..(ms / 50) {
        ctx.settle(50);
    }
}

/// The chrome tokens the window paints with (`theme.tokens`).
fn tokens(app: &mut App) -> Value {
    probe(app, "theme.tokens", Value::Null)
}

/// One token as its `#rrggbbaa` string, or "" when it is not there.
fn tok(app: &mut App, key: &str) -> String {
    tokens(app)[key].as_str().unwrap_or("").to_string()
}

/// The alpha of a `#rrggbb[aa]` string, 0 to 1 (1 when the string has no alpha).
fn alpha_of(s: &str) -> f64 {
    let h = s.trim_start_matches('#');
    if h.len() >= 8 {
        u8::from_str_radix(&h[6..8], 16).map_or(1.0, |a| f64::from(a) / 255.0)
    } else {
        1.0
    }
}

/// The rgb of a `#rrggbb[aa]` string as three numbers.
fn rgb_of(s: &str) -> [f64; 3] {
    let h = s.trim_start_matches('#');
    let ch = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    [f64::from(ch(0)), f64::from(ch(2)), f64::from(ch(4))]
}

/// The lower-case `#rrggbb` part of a `#rrggbb[aa]` string.
fn hex6(s: &str) -> String {
    s.trim().to_lowercase().chars().take(7).collect()
}

/// The stored settings (`<profile>\settings.json`, a JSON object), or an empty object.
fn store(profile: &Path) -> Value {
    let text = std::fs::read_to_string(profile.join("settings.json")).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_else(|_| json!({}))
}

/// One stored key as a string, or None when nothing is stored under it.
fn stored(profile: &Path, key: &str) -> Option<String> {
    store(profile)[key].as_str().map(str::to_string)
}

/// Writes (merges) settings into a closed profile, so the next launch starts from them.
fn seed(profile: &Path, pairs: &[(&str, &str)]) -> Result<(), String> {
    let mut map: BTreeMap<String, String> = store(profile)
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    for (k, v) in pairs {
        map.insert((*k).to_string(), (*v).to_string());
    }
    std::fs::create_dir_all(profile).map_err(|e| e.to_string())?;
    let text = serde_json::to_string(&map).map_err(|e| e.to_string())?;
    std::fs::write(profile.join("settings.json"), text).map_err(|e| e.to_string())
}

/// The wall, as `picker.wall` answers it on the page in front.
fn wall(app: &mut App) -> Value {
    probe(app, "picker.wall", Value::Null)
}

/// The wall's cards in order, as (id, rect, on, visible).
fn wall_cards(app: &mut App) -> Vec<(String, R, bool, bool)> {
    wall(app)["cards"]
        .as_array()
        .map(|cards| {
            cards
                .iter()
                .filter_map(|c| {
                    Some((
                        c["id"].as_str()?.to_string(),
                        rect_of(&c["rect"])?,
                        c["on"] == json!(true),
                        c["visible"] == json!(true),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Opens the wall (its toggle), brings a card into view and clicks it; true when it is the theme in
/// force afterwards. The wheel steps the pane the way `reveal` does.
fn pick_card(app: &mut App, id: &str) -> bool {
    if wall(app)["expanded"] == json!(false) {
        let toggle = wall(app)["toggle"].clone();
        click_rect(app, &toggle);
        wait_for(4_000, &mut || wall(app)["expanded"] == json!(true));
    }
    let at = pane_point(app);
    let in_view = wait_for(12_000, &mut || {
        let Some(c) = wall_cards(app).into_iter().find(|c| c.0 == id) else {
            return false;
        };
        if c.3 {
            return true;
        }
        let mid = ui_rect(app, "grid");
        let mid = rect_of(&mid)
            .map(|(_, gy, _, gh)| gy + gh / 2.0)
            .unwrap_or(300.0);
        let dy = if c.1 .1 > mid { -160.0 } else { 160.0 };
        let _ = app.session.call(&Request::Pointer {
            x: at.0,
            y: at.1,
            action: PointerAction::Wheel { dx: 0.0, dy },
        });
        false
    });
    if !in_view {
        return false;
    }
    let Some(c) = wall_cards(app).into_iter().find(|c| c.0 == id) else {
        return false;
    };
    let (x, y) = centre(c.1);
    let _ = app.session.click(x, y);
    // A pick over unsaved changes asks first: Discard drops them (settings_b's route, Tab twice from
    // Save as Custom, then Enter), as PT's accent scenario does.
    if wait_for(2_000, &mut || ask_open(app)) {
        let _ = app.session.key("Tab");
        let _ = app.session.key("Tab");
        let _ = app.session.key("Enter");
        let _ = wait_for(3_000, &mut || !ask_open(app));
    }
    let got = wait_for(4_000, &mut || {
        wall_cards(app).iter().any(|c| c.0 == id && c.2)
    });
    got
}

/// Whether the switch question is open.
fn ask_open(app: &mut App) -> bool {
    probe(app, "settings.theme", Value::Null)["ask"]["open"] == json!(true)
}

/// The first `n` ids of the wall, minus `custom` (PT's `wallOrder`).
fn wall_ids(app: &mut App) -> Vec<String> {
    wall_cards(app)
        .into_iter()
        .map(|c| c.0)
        .filter(|id| id != "custom")
        .collect()
}

/// The order the wall must run in: PT Default first, then the oracle's order (Flare's
/// `wall_order.json` fixture, the same ids PT's snapshot holds).
fn expected_wall_order() -> Result<Vec<String>, String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/flare-theme/tests/fixtures/wall_order.json"
    );
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let ids: Vec<String> = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    let mut out = vec!["pt-default".to_string()];
    out.extend(ids.into_iter().filter(|i| i != "pt-default"));
    Ok(out)
}

/// The knob rule of PT's `knobRule`, ported: the theme's ink for a switch's knob on this ground,
/// unless that ink reads under 2:1 on the track, when the opposite ink is used. Returns (want, dark,
/// ratio of the ink on the track, the track as seen over the ground).
fn knob_rule(ground: [f64; 3], track: &str) -> ([f64; 3], bool, f64, [f64; 3]) {
    let a = alpha_of(track);
    let t = rgb_of(track);
    // Each channel is rounded to a whole number, as the chrome's composite is a hex string (switch_knob
    // reads that), so a track on the 2:1 line decides the same way as the window does.
    let seen = [0, 1, 2].map(|i| (ground[i] + (t[i] - ground[i]) * a).round());
    let dark = lum(ground) <= 0.4;
    let ink = if dark { BLACK } else { WHITE };
    let r = ratio(ink, seen);
    let want = if r >= 2.0 {
        ink
    } else if dark {
        WHITE
    } else {
        BLACK
    };
    (want, dark, r, seen)
}

/// WCAG relative luminance of an rgb triple (0 to 255 channels).
fn lum(c: [f64; 3]) -> f64 {
    let lin = |v: f64| {
        let v = v / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// The WCAG contrast ratio of two rgb triples.
fn ratio(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn rgb_eq(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 0.5)
}

/// The ground the knob is judged on: the solid ground token, as rgb.
fn ground_rgb(app: &mut App) -> [f64; 3] {
    rgb_of(&tok(app, "ground_solid"))
}

/// The two checks PT's `knobOk` makes for one on switch: the knob is the theme's ink (or the
/// opposite under 2:1), and it reads at least 2:1 on the track. Returns the rule's answer.
fn knob_ok(ctx: &mut Ctx, app: &mut App, tag: &str) -> ([f64; 3], bool, f64) {
    let t = tokens(app);
    let knob = rgb_of(t["switch_knob"].as_str().unwrap_or(""));
    let track = t["sel_bg"].as_str().unwrap_or("").to_string();
    let ground = rgb_of(t["ground_solid"].as_str().unwrap_or(""));
    let (want, dark, r, seen) = knob_rule(ground, &track);
    let name = |c: [f64; 3]| if rgb_eq(c, BLACK) { "black" } else { "white" };
    ctx.ok(
        rgb_eq(knob, want),
        format!(
            "{tag}: the knob is {} on a {} theme (the theme's ink at {:.2}:1 on the track)",
            name(want),
            if dark { "dark" } else { "light" },
            r
        ),
    );
    let on_track = ratio(knob, seen);
    ctx.ok(
        on_track >= 2.0,
        format!("{tag}: and it reads at least 2:1 on the track ({on_track:.2}:1)"),
    );
    (knob, dark, r)
}

/// A colour code as the row shows it (`#e07a2f`, `e07a2f`, or with alpha) in lower case, six digits.
fn code6(s: &str) -> String {
    s.trim()
        .trim_start_matches('#')
        .chars()
        .take(6)
        .collect::<String>()
        .to_lowercase()
}

/// Launches the app on the alpha folder, with the software renderer.
fn launch_one(ctx: &mut Ctx) -> Result<App, String> {
    let alpha = ctx.ensure_world().alpha.clone();
    ctx.launch(Launch {
        folders: vec![alpha],
        renderer: Some("software"),
        ..Launch::default()
    })
}

/// Forgets the last session's tabs, so a launch that is meant to start with two folders gets two
/// tabs and no restored ones (the app restores tabs.json on every start).
fn fresh_tabs(profile: &Path) {
    let _ = std::fs::remove_file(profile.join("tabs.json"));
}

/// Launches the app on the alpha and beta folders: two tabs, so one is the active tab.
fn launch_two(ctx: &mut Ctx) -> Result<App, String> {
    let (alpha, beta) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.beta.clone())
    };
    ctx.launch(Launch {
        folders: vec![alpha, beta],
        renderer: Some("software"),
        ..Launch::default()
    })
}

// ---- themeCards ----------------------------------------------------------------------------------------

/// One measurement of the wall: each card's id, its top in the page (scroll taken off) and its
/// height, and the top of the row below the wall in the page.
struct WallPage {
    cards: Vec<(String, f64, f64)>,
    below: f64,
}

fn measure_wall(app: &mut App) -> WallPage {
    let scroll = probe(app, "picker.row", json!({ "id": "window-background" }))["scroll_y"]
        .as_f64()
        .unwrap_or(0.0);
    let below = rect_of(&probe(app, "picker.row", json!({ "id": "window-background" }))["rect"])
        .map(|r| r.1 + scroll)
        .unwrap_or(-1.0);
    let cards = wall_cards(app)
        .into_iter()
        .map(|c| (c.0, c.1 .1 + scroll, c.1 .3))
        .collect();
    WallPage { cards, below }
}

/// themeCards (PT run.mjs:6580): a pick moves nothing; the wall runs in the order the snapshot
/// holds; a plain Reset word puts the theme's colour back.
pub fn theme_cards(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = launch_one(ctx)?;
    if !wait_for(15_000, &mut || tabs(&mut app).len() == 1) {
        ctx.ok(false, "one tab open");
        app.close();
        return Ok(());
    }
    if !open_settings(&mut app) || !goto_page(&mut app, "appearance") {
        ctx.ok(false, "Settings opens on the Appearance page");
        app.close();
        return Ok(());
    }
    wait_for(10_000, &mut || !wall_cards(&mut app).is_empty());

    // THE ORDER (#157): PT Default leads, then the neutrals, then the coloured ones.
    let want = expected_wall_order()?;
    let got = wall_ids(&mut app);
    ctx.ok(
        want.len() >= 39 && got == want,
        format!(
            "the wall runs PT Default, neutrals black to white, then coloured ({} ...)",
            got.iter().take(14).cloned().collect::<Vec<_>>().join(" ")
        ),
    );

    // Two rows to pick across.
    let first = measure_wall(&mut app);
    let mut tops: Vec<f64> = Vec::new();
    for (_, top, _) in &first.cards {
        if !tops.iter().any(|t| (t - top).abs() < 0.5) {
            tops.push(*top);
        }
    }
    ctx.ok(
        tops.len() >= 2,
        format!("the wall has two rows to pick across ({})", tops.len()),
    );
    let in_row = |top: f64| {
        first
            .cards
            .iter()
            .find(|c| (c.1 - top).abs() < 0.5)
            .map(|c| c.0.clone())
    };
    let picks: Vec<String> = match (tops.first(), tops.get(1)) {
        (Some(a), Some(b)) => [in_row(*a), in_row(*b)].into_iter().flatten().collect(),
        _ => Vec::new(),
    };
    let mut seen: Vec<WallPage> = Vec::new();
    for id in &picks {
        pick_card(&mut app, id);
        hold(ctx, 500);
        seen.push(measure_wall(&mut app));
    }
    if seen.len() < 2 {
        ctx.ok(false, "two picks were made across the two rows");
        app.close();
        return Ok(());
    }

    // Every card is one height, selected or not.
    let mut heights: Vec<String> = seen
        .iter()
        .flat_map(|m| m.cards.iter().map(|c| format!("{:.2}", c.2)))
        .collect();
    heights.sort();
    heights.dedup();
    ctx.ok(
        heights.len() == 1,
        format!(
            "every card is one height, selected or not ({})",
            heights.join(" / ")
        ),
    );

    // The shift the owner saw is the SECOND row moving.
    let row2 = |m: &WallPage| m.cards.iter().find(|c| c.0 == picks[1]).map(|c| c.1);
    ctx.ok(
        row2(&seen[0]).is_some() && row2(&seen[0]) == row2(&seen[1]),
        format!(
            "the second row stays put when the pick moves between rows ({:?} -> {:?})",
            row2(&seen[0]),
            row2(&seen[1])
        ),
    );
    ctx.ok(
        (seen[0].below - seen[1].below).abs() < 0.5 && seen[0].below >= 0.0,
        format!(
            "and nothing below the wall moves ({} -> {})",
            seen[0].below, seen[1].below
        ),
    );

    // A colour put back to the theme's is a plain Reset word (the link's own look is not probed).
    goto_row(&mut app, "agent-color");
    let themed = code6(
        row(&mut app, "agent-color")["colour"]
            .as_str()
            .unwrap_or(""),
    );
    fill_row_colour(&mut app, "agent-color", "#e07a2f");
    let offered = wait_for(5_000, &mut || {
        row(&mut app, "agent-color")["reset"] == json!(true)
    });
    ctx.ok(
        offered,
        "a picked colour offers a plain \"Reset\" word (the link's border and fill are not probed)",
    );
    press_reset(&mut app, "agent-color");
    let back = wait_for(5_000, &mut || {
        let r = row(&mut app, "agent-color");
        code6(r["colour"].as_str().unwrap_or("")) == themed && r["reset"] != json!(true)
    });
    ctx.ok(back, "and Reset puts the theme's colour back and goes away");
    app.close();
    Ok(())
}

// ---- edges ---------------------------------------------------------------------------------------------

/// The four edges as PT names them, each with the token that draws it in Flare: the title bar's and
/// the rail's rule are the divider, the panel's and the rule between rows are the list's line.
#[derive(Debug, Clone, Default)]
struct Edges {
    title: f64,
    rail: f64,
    row: f64,
    rule: f64,
    divider: String,
    line: String,
}

fn measure_edges(app: &mut App) -> Edges {
    let divider = tok(app, "divider");
    let line = tok(app, "line");
    Edges {
        title: alpha_of(&divider),
        rail: alpha_of(&divider),
        row: alpha_of(&line),
        rule: alpha_of(&line),
        divider,
        line,
    }
}

/// Waits until the edges wear the state asked for; the measurement that satisfied it, or None.
fn settled_edges(app: &mut App, want: &dyn Fn(&Edges) -> bool) -> Option<Edges> {
    let mut got: Option<Edges> = None;
    let ok = wait_for(15_000, &mut || {
        let m = measure_edges(app);
        if want(&m) {
            got = Some(m);
            true
        } else {
            false
        }
    });
    if ok {
        got
    } else {
        None
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < NEAR
}

/// Holds a scenario's launch two tabs open and returns the tab width sum and strip height (the
/// layout a border would move), so the check can see that no option moves anything.
fn layout_of(app: &mut App) -> String {
    let widths: Vec<String> = tabs(app)
        .iter()
        .map(|t| format!("{:.2}", t["rect"]["w"].as_f64().unwrap_or(0.0)))
        .collect();
    let strip = ui_rect(app, "strip");
    format!(
        "{}|{:.2}",
        widths.join(","),
        strip["h"].as_f64().unwrap_or(0.0)
    )
}

/// edges (PT run.mjs:6683): None, Faint, Hairline and Solid each stored and applied; None is
/// transparent; a dark Solid is white ink and a light Solid black at 26%; a relaunch keeps Solid.
pub fn edges(ctx: &mut Ctx) -> Result<(), String> {
    let profile = ctx.ensure_world().profile.clone();
    let mut app = launch_two(ctx)?;
    // Two tabs, so there IS a line between tabs to measure.
    let two = wait_for(15_000, &mut || tabs(&mut app).len() == 2);
    ctx.ok(two, "two tabs open");

    // THE DEFAULT IS THE WINDOW AS IT WAS: the numbers chromeTheme hard-coded before there was a choice.
    let first = settled_edges(&mut app, &|m| near(m.title, 0.07) && near(m.row, 0.09));
    ctx.ok(first.is_some(), "at launch every edge wears its token");
    ctx.ok(
        stored(&profile, "flare.window.edges").is_none(),
        "nothing is stored until somebody chooses",
    );
    let first = first.unwrap_or_default();
    ctx.ok(
        first.divider.eq_ignore_ascii_case("#ffffff12")
            && first.line.eq_ignore_ascii_case("#ffffff17"),
        format!(
            "the default is exactly the look before the setting existed ({}, {})",
            first.divider, first.line
        ),
    );
    ctx.ok(
        near(first.title, 0.07),
        format!(
            "the title bar's rule is the 7% hairline it always was ({:.3})",
            first.title
        ),
    );
    // NO LINE BETWEEN TABS (#143): Flare's strip draws no separator, and no probe reads a tab's
    // border, so this is not checked here (PT's `tabBorder`).
    ctx.skip("DROPPED: no line between tabs (PT tabBorder): Flare's strip draws no separator or tab border by design, so there is no line to measure");
    let main_hairline = probe(&mut app, "window.edge", json!({ "edges": "hairline" }));
    ctx.ok(
        main_hairline.is_string(),
        format!(
            "main holds a hairline for the window border (PROXY: window.edge = {main_hairline})"
        ),
    );

    open_settings(&mut app);
    goto_page(&mut app, "appearance");
    reveal(&mut app, "window-edges");
    let r = row(&mut app, "window-edges");
    let names: Vec<String> = r["options"]
        .as_array()
        .map(|o| {
            o.iter()
                .filter_map(|n| n.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    ctx.ok(
        names.join("|") == "None|Faint|Hairline|Solid",
        "Settings > Appearance has an Edges row, weakest to strongest as in PT: None, Faint, Hairline, Solid",
    );
    ctx.ok(
        r["value"].as_str() == Some("hairline"),
        format!(
            "with Hairline pressed, since that is what is in force ({})",
            r["value"]
        ),
    );
    let now = measure_edges(&mut app);
    ctx.ok(
        near(now.row, 0.09) && near(now.rule, 0.09),
        "a settings panel and the hairline between its rows wear the 9% list line",
    );
    app.close();

    // Each option, written and then the app started again: the segment's own click is not probed.
    let mut seen: BTreeMap<&str, Edges> = BTreeMap::new();
    let mut layouts: Vec<String> = Vec::new();
    for id in ["faint", "none", "hairline", "solid"] {
        seed(&profile, &[("flare.window.edges", id)])?;
        fresh_tabs(&profile);
        let mut a = launch_two(ctx)?;
        let p = settled_edges(&mut a, &|m| {
            if id == "none" {
                m.title == 0.0 && m.row == 0.0
            } else {
                m.title > 0.0 && m.row > 0.0
            }
        });
        ctx.ok(
            p.is_some(),
            format!("{id}: picked, stored, and every edge has arrived at its token"),
        );
        if let Some(p) = p {
            layouts.push(layout_of(&mut a));
            seen.insert(id, p);
        }
        let main_says = probe(&mut a, "window.edge", json!({ "edges": id }));
        let want_null = id == "none";
        ctx.ok(
            main_says.is_null() == want_null,
            format!("{id}: main was told, for the border round the window ({main_says})"),
        );
        a.close();
    }

    let all = ["none", "faint", "hairline", "solid"]
        .iter()
        .all(|id| seen.contains_key(id));
    ctx.ok(all, "all four options were measured");
    if all {
        let (none, faint, hairline, solid) = (
            &seen["none"],
            &seen["faint"],
            &seen["hairline"],
            &seen["solid"],
        );
        for edge in ["title", "rail", "row", "rule"] {
            let v = |e: &Edges| match edge {
                "title" => e.title,
                "rail" => e.rail,
                "row" => e.row,
                _ => e.rule,
            };
            ctx.ok(
                v(none) == 0.0,
                format!("none: the {edge} edge is transparent (alpha {})", v(none)),
            );
        }
        for edge in ["title", "rail", "row", "rule"] {
            let v = |e: &Edges| match edge {
                "title" => e.title,
                "rail" => e.rail,
                "row" => e.row,
                _ => e.rule,
            };
            ctx.ok(
                v(faint) > 0.0 && v(faint) < v(hairline) && v(hairline) < v(solid),
                format!(
                    "{edge}: faint < hairline < solid ({} < {} < {})",
                    v(faint),
                    v(hairline),
                    v(solid)
                ),
            );
        }
        ctx.ok(
            near(hairline.title, 0.07) && near(hairline.row, 0.09),
            "going back to Hairline is going back to the old look",
        );
        ctx.ok(
            rgb_eq(rgb_of(&solid.divider), WHITE),
            format!("on a dark ground the line is white ink ({})", solid.divider),
        );
        // Transparent, not absent: nothing moves between the options.
        let mut uniq = layouts.clone();
        uniq.sort();
        uniq.dedup();
        ctx.ok(
            uniq.len() == 1,
            format!("no option moves the layout ({})", uniq.join(" ; ")),
        );
    }

    // A light theme: the same choice, in black ink at the light ground's alpha. Solid is the last
    // option written, so that is what the window holds now.
    fresh_tabs(&profile);
    let mut a = launch_two(ctx)?;
    if !wait_for(15_000, &mut || tabs(&mut a).len() == 2) {
        ctx.ok(false, "the app is up for the light theme");
        a.close();
        return Ok(());
    }
    open_settings(&mut a);
    goto_page(&mut a, "appearance");
    pick_card(&mut a, "paper");
    let light = settled_edges(&mut a, &|m| {
        rgb_eq(rgb_of(&m.divider), [0.0; 3]) && near(m.title, 0.26)
    });
    ctx.ok(
        light.is_some(),
        format!(
            "on a light theme Solid is black ink at 26% ({} @ {:.3})",
            tok(&mut a, "divider"),
            alpha_of(&tok(&mut a, "divider"))
        ),
    );

    // AND IT SURVIVES A RELAUNCH: the app quits when asked, then comes back with Solid and the light theme.
    let quit = a.close();
    ctx.ok(
        quit,
        "the app quits when it is asked to, before anything relaunches it",
    );
    let mut b = ctx.launch(Launch {
        renderer: Some("software"),
        ..Launch::default()
    })?;
    let back_tabs = wait_for(20_000, &mut || tabs(&mut b).len() == 2);
    ctx.ok(back_tabs, "a relaunch brings the tabs back");
    let back = settled_edges(&mut b, &|m| {
        near(m.title, 0.26) && stored(&profile, "flare.window.edges").as_deref() == Some("solid")
    });
    ctx.ok(
        back.is_some(),
        format!(
            "the relaunched window draws Solid edges before Settings is opened (stored {:?})",
            stored(&profile, "flare.window.edges")
        ),
    );
    let told = probe(&mut b, "window.edge", json!({ "edges": "solid" }));
    ctx.ok(
        told.is_string(),
        "and main was told again at launch (PROXY: window.edge for solid)",
    );
    open_settings(&mut b);
    goto_page(&mut b, "appearance");
    let pressed = wait_for(10_000, &mut || {
        row(&mut b, "window-edges")["value"].as_str() == Some("solid")
    });
    ctx.ok(pressed, "and the row shows Solid pressed");
    let regs = probe(&mut b, "registry.writes", Value::Null);
    ctx.ok(
        regs["count"] == json!(0),
        "no registry write was attempted under --e2e",
    );
    b.close();
    Ok(())
}

// ---- accent ----------------------------------------------------------------------------------------------

/// accent (PT run.mjs:6864): the active tab's rule is the accent; the settings controls do not wear
/// it; an on switch's track is the accent fill and its knob follows the theme; a pick stores and
/// offers Reset; hex8 gives a see-through fill with an opaque rule and line; Reset puts it back.
pub fn accent(ctx: &mut Ctx) -> Result<(), String> {
    let profile = ctx.ensure_world().profile.clone();
    let mut app = launch_two(ctx)?;
    let two = wait_for(15_000, &mut || tabs(&mut app).len() == 2);
    ctx.ok(two, "two tabs open, so one is the active tab");

    let before_accent = tok(&mut app, "accent");
    let before_hi = tok(&mut app, "accent_hi");
    // The tab's rule is drawn with the accent-hi token (tabs.slint); PROXY for the DOM rule's colour.
    ctx.ok(
        !before_hi.is_empty() && alpha_of(&before_hi) == 1.0,
        format!("the active tab wears the accent rule ({before_hi}, the accent-hi token)"),
    );
    open_settings(&mut app);
    goto_page(&mut app, "appearance");

    // SETTINGS CONTROLS DO NOT WEAR THE ACCENT (2026-09-23). Sampled with the theme's accent, and
    // again after a pick: the segment's fill and the control's fill must not move. An on switch's
    // track is the accent fill (2026-10-07), and its knob follows the theme (2026-10-10).
    // The Appearance page has no on switch row (its rows are segments and pickers), so the pressed
    // segment is the row check here; the on switch is read from its token below (PROXY).
    let pressed_segment = row(&mut app, "tab-layout")["value"]
        .as_str()
        .unwrap_or("")
        .to_string();
    ctx.ok(
        !pressed_segment.is_empty(),
        format!("a pressed segment is on the Appearance page ({pressed_segment}; the on switch is read from its token)"),
    );
    let plain_control = tok(&mut app, "control");
    let plain_pressed = tok(&mut app, "pressed");
    let plain_track = tok(&mut app, "sel_bg");
    ctx.ok(
        hex6(&plain_track) == hex6(&before_accent),
        format!(
            "an on switch's track is the theme's accent fill ({plain_track} vs {before_accent})"
        ),
    );
    knob_ok(ctx, &mut app, "PT Default");

    // Nothing is chosen at first, so there is nothing to follow back to.
    let idle_row = row(&mut app, "window-accent");
    ctx.ok(
        stored(&profile, "flare.window.accent").is_none() && idle_row["reset"] != json!(true),
        "nothing is chosen at first, so there is nothing to follow back to",
    );
    ctx.ok(
        code6(idle_row["code"].as_str().unwrap_or("")) == code6(&before_accent),
        format!(
            "the swatch shows the theme's own accent ({} vs {before_accent})",
            idle_row["code"]
        ),
    );

    // A pick: the accent, and the tab's rule follows it.
    fill_row_colour(&mut app, "window-accent", "#E07A2F");
    let picked = wait_for(10_000, &mut || {
        tok(&mut app, "accent").eq_ignore_ascii_case("#e07a2fff")
            && tok(&mut app, "accent_hi") != before_hi
    });
    ctx.ok(
        picked,
        format!(
            "a picked colour is the accent, and the tab's rule follows it ({})",
            tok(&mut app, "accent_hi")
        ),
    );
    wait_for(4_000, &mut || {
        stored(&profile, "flare.window.accent").as_deref() == Some("#e07a2f")
    });
    ctx.ok(
        stored(&profile, "flare.window.accent").as_deref() == Some("#e07a2f")
            && row(&mut app, "window-accent")["reset"] == json!(true),
        format!(
            "it is stored, and Reset is offered (stored {:?}, reset {})",
            stored(&profile, "flare.window.accent"),
            row(&mut app, "window-accent")["reset"]
        ),
    );

    // A SEE-THROUGH ACCENT (#114): a hex8 accent publishes a fill with alpha; the line token is the
    // exact pick, opaque.
    fill_row_colour(&mut app, "window-accent", "#e07a2f80");
    let glass_accent = wait_for(8_000, &mut || {
        tok(&mut app, "accent").eq_ignore_ascii_case("#e07a2f80")
            && stored(&profile, "flare.window.accent").as_deref() == Some("#e07a2f80")
    });
    ctx.ok(
        glass_accent,
        format!(
            "a hex8 accent publishes a see-through fill ({})",
            tok(&mut app, "accent")
        ),
    );
    ctx.ok(
        alpha_of(&tok(&mut app, "accent_hi")) == 1.0,
        format!(
            "and the active tab's rule stays solid ({})",
            tok(&mut app, "accent_hi")
        ),
    );
    ctx.ok(
        tok(&mut app, "accent_solid").eq_ignore_ascii_case("#e07a2fff"),
        format!(
            "the accent's line token is the pick, opaque ({})",
            tok(&mut app, "accent_solid")
        ),
    );
    ctx.ok(
        alpha_of(&tok(&mut app, "accent_solid")) == 1.0
            && rgb_eq(rgb_of(&tok(&mut app, "accent_solid")), [224.0, 122.0, 47.0]),
        "and the tab spinner's ring drawn from it is solid",
    );

    // Save changes wears the accent fill. PT reads the painted pixels (GPU only); this reads the
    // token the fill and the label are drawn with (a token-level PROXY).
    let accent_now = tok(&mut app, "accent");
    let ground = ground_rgb(&mut app);
    let composite = |c: &str| -> [f64; 3] {
        let a = alpha_of(c);
        let f = rgb_of(c);
        [0, 1, 2].map(|i| ground[i] + (f[i] - ground[i]) * a)
    };
    let fill_alpha = alpha_of(&accent_now);
    ctx.ok(
        (fill_alpha - 128.0 / 255.0).abs() < 0.01,
        format!("over an opaque window the Save fill is see-through (token alpha {fill_alpha:.2})"),
    );
    let label = rgb_of(&tok(&mut app, "on_accent"));
    let reads = ratio(label, composite(&accent_now));
    ctx.ok(
        reads >= 4.5,
        format!("and its label reads on the composite (PROXY {reads:.1}:1)"),
    );

    // Under a see-through window (the slider is GPU only): skipped on software, never passed.
    ctx.skip(SOFTWARE_SKIP);
    ctx.skip(SOFTWARE_SKIP);

    // The settings controls keep their own fills: none of them is the accent.
    let after_control = tok(&mut app, "control");
    let after_pressed = tok(&mut app, "pressed");
    ctx.ok(
        after_control == plain_control,
        format!("the control is unchanged by the picked accent ({after_control})"),
    );
    ctx.ok(
        hex6(&after_control) != hex6(&accent_now) && hex6(&after_pressed) != hex6(&accent_now),
        "and the control and a pressed segment wear no accent",
    );
    ctx.ok(
        after_pressed == plain_pressed,
        format!("the segment is unchanged by the picked accent ({after_pressed})"),
    );
    let after_track = tok(&mut app, "sel_bg");
    ctx.ok(
        after_track != plain_track && hex6(&after_track) == hex6(&accent_now),
        format!("an on switch's track follows the picked accent ({plain_track} -> {after_track})"),
    );
    knob_ok(ctx, &mut app, "a picked accent");

    // Reset puts back exactly the theme's accent, and forgets the choice.
    press_reset(&mut app, "window-accent");
    let back = wait_for(10_000, &mut || {
        hex6(&tok(&mut app, "accent")) == hex6(&before_accent)
            && tok(&mut app, "accent_hi") == before_hi
    });
    ctx.ok(
        back,
        format!("Reset puts back exactly the theme's accent ({before_accent})"),
    );
    wait_for(4_000, &mut || {
        stored(&profile, "flare.window.accent").is_none()
    });
    ctx.ok(
        stored(&profile, "flare.window.accent").is_none()
            && row(&mut app, "window-accent")["reset"] != json!(true),
        format!(
            "and forgets the choice (stored {:?}, reset {})",
            stored(&profile, "flare.window.accent"),
            row(&mut app, "window-accent")["reset"]
        ),
    );

    // THE DEFAULT THEME IS PT DEFAULT: a fresh profile has it selected, and it leads the wall.
    let cards = wall_cards(&mut app);
    ctx.ok(
        cards.iter().any(|c| c.0 == "pt-default" && c.2),
        "a fresh profile wears PT Default, the default theme",
    );
    ctx.ok(
        cards.first().map(|c| c.0.as_str()) == Some("pt-default"),
        "and it is the first card in the wall",
    );

    // BACKGROUND AND ACCENT SIT RIGHT UNDER THE THEME WALL, after See-through.
    let order = page_rows(&mut app);
    let at = order.iter().position(|id| id == "term-acrylic");
    let in_order = at.is_some_and(|i| {
        order.get(i + 1).map(String::as_str) == Some("window-background")
            && order.get(i + 2).map(String::as_str) == Some("window-secondary")
            && order.get(i + 3).map(String::as_str) == Some("window-accent")
    });
    ctx.ok(
        in_order,
        format!(
            "See-through, Background, Secondary and Accent come right after the theme wall ({})",
            order.join(" > ")
        ),
    );

    // THE BACKGROUND: the window's ground, one colour; Reset puts it back.
    let theme_ground = tok(&mut app, "ground_solid");
    let shown_ground = row(&mut app, "window-background")["code"]
        .as_str()
        .unwrap_or("")
        .to_string();
    ctx.ok(
        code6(&shown_ground) == code6(&theme_ground),
        format!("the background swatch shows the theme's own ({theme_ground} vs {shown_ground})"),
    );
    fill_row_colour(&mut app, "window-background", "#1c2330");
    let painted = wait_for(8_000, &mut || {
        hex6(&tok(&mut app, "ground_solid")) == "#1c2330"
            && stored(&profile, "flare.window.background").as_deref() == Some("#1c2330")
            && row(&mut app, "window-background")["reset"] == json!(true)
    });
    ctx.ok(
        painted,
        "a picked background is the window ground, stored, with Reset offered",
    );
    press_reset(&mut app, "window-background");
    let ground_back = wait_for(8_000, &mut || {
        hex6(&tok(&mut app, "ground_solid")) == hex6(&theme_ground)
            && stored(&profile, "flare.window.background").is_none()
            && row(&mut app, "window-background")["reset"] != json!(true)
    });
    ctx.ok(ground_back, "Reset puts the theme's background back");

    // On a LIGHT theme the on switch is that theme's accent and its ink (#138).
    // PT waits 900 ms for the strip's colour transitions (550 ms) before it clicks a card. Without that
    // wait the first click lands during the transition and selects nothing (seen 2 runs in 3).
    hold(ctx, 900);
    let paper_picked = pick_card(&mut app, "paper");
    let light_track = tok(&mut app, "sel_bg");
    let light_accent = tok(&mut app, "accent");
    ctx.ok(
        hex6(&light_track) == hex6(&light_accent) && hex6(&light_track) != hex6(&plain_track),
        format!(
            "on a light theme the on switch is its accent ({light_track}; Paper picked {paper_picked}, light {})",
            tokens(&mut app)["light"]
        ),
    );
    knob_ok(ctx, &mut app, "Paper");

    // THE FLIP, "only when the color is very close": a very light accent on Paper goes black,
    // a very dark accent on PT Default goes white.
    let accent_then = |ctx: &mut Ctx, app: &mut App, hex: &str| -> ([f64; 3], bool, f64) {
        fill_row_colour(app, "window-accent", hex);
        wait_for(6_000, &mut || {
            stored(&profile, "flare.window.accent").as_deref() == Some(hex)
        });
        hold(ctx, 200);
        let knob = rgb_of(&tok(app, "switch_knob"));
        let (_, dark, r, _) = knob_rule(ground_rgb(app), &tok(app, "sel_bg"));
        (knob, dark, r)
    };
    let (pale_knob, _, pale_r) = {
        let (k, d, r) = accent_then(ctx, &mut app, "#cfe4ff80");
        let _ = knob_ok(ctx, &mut app, "Paper, a very light accent");
        (k, d, r)
    };
    ctx.ok(
        rgb_eq(pale_knob, BLACK) && pale_r < 2.0,
        format!("there it flips to black: white would read {pale_r:.2}:1"),
    );
    // Back to PT Default first: the switch asks about the picked accent and Discard lets it go (PT's
    // accent scenario does the same), so the accent the deep checks pick is not the see-through one.
    pick_card(&mut app, "pt-default");
    // The picked accent is a change to save: the switch asks, and Discard lets it go.
    fill_row_colour(&mut app, "window-accent", "#2a2c30");
    hold(ctx, 200);
    let deep_solid = rgb_of(&tok(&mut app, "switch_knob"));
    knob_ok(ctx, &mut app, "PT Default, a very dark opaque accent");
    ctx.ok(
        rgb_eq(deep_solid, BLACK),
        format!(
            "an opaque one is floored off the ground and keeps the black knob (accent {}, track {}, knob {})",
            tok(&mut app, "accent"),
            tok(&mut app, "sel_bg"),
            tok(&mut app, "switch_knob")
        ),
    );
    let (deep_knob, _, deep_r) = accent_then(ctx, &mut app, "#2a2c3080");
    let _ = knob_ok(ctx, &mut app, "PT Default, a very dark accent");
    ctx.ok(
        rgb_eq(deep_knob, WHITE) && deep_r < 2.0,
        format!("there it flips to white: black would read {deep_r:.2}:1"),
    );
    app.close();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dark_ground_with_an_orange_track_takes_a_black_knob() {
        // PT Default: the ground is near black, the track the theme's orange, opaque.
        let (want, dark, r, _) = knob_rule([23.0, 23.0, 23.0], "#fe8f34ff");
        assert!(dark);
        assert!(r >= 2.0);
        assert!(rgb_eq(want, BLACK));
    }

    #[test]
    fn the_ratio_of_pt_black_and_white_is_about_nineteen_and_a_half() {
        // BLACK is PT's #0b0b0f, not pure black, so the ratio sits just under 21:1.
        let r = ratio(BLACK, WHITE);
        assert!((r - 19.6).abs() < 0.2, "{r}");
        assert!(ratio([0.0; 3], WHITE) > 20.9);
    }

    #[test]
    fn alpha_and_rgb_of_a_hex8() {
        assert!((alpha_of("#e07a2f80") - 128.0 / 255.0).abs() < 1e-9);
        assert_eq!(alpha_of("#e07a2f"), 1.0);
        assert_eq!(rgb_of("#e07a2f80"), [224.0, 122.0, 47.0]);
    }
}
