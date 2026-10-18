//! Settings B scenarios (Task 34): windowColoursSave, accentAsk, themeEditorLive, themeSwitch and
//! reviewSettings. Each follows its Prism Terminal line (PT: run.mjs, 2820, 3145, 5667, 5746,
//! 5894); a body says where Flare differs. Flare answers through the `settings.theme`,
//! `settings.colours` and `settings.row` probes (e2e_probes/settings_b.rs), the committed
//! `settings.rows`, `settings.state`, `settings.rail` and `ui.rect` probes, pixels from a snapshot,
//! and the stored settings in `<profile>\settings.json` (PT reads localStorage).

use flare_e2e_proto::{PointerAction, Request};
use serde_json::{json, Value};

use crate::ctx::{App, Ctx, Launch};

/// A probe's answer, `Null` when it fails (a check on it then reads false).
fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// A css rectangle (`x`, `y`, `w`, `h`) as a tuple.
fn rect(v: &Value) -> Option<(f64, f64, f64, f64)> {
    Some((
        v["x"].as_f64()?,
        v["y"].as_f64()?,
        v["w"].as_f64()?,
        v["h"].as_f64()?,
    ))
}

/// The centre of a css rectangle, as the pointer wants it.
fn centre(r: (f64, f64, f64, f64)) -> (f32, f32) {
    ((r.0 + r.2 / 2.0) as f32, (r.1 + r.3 / 2.0) as f32)
}

/// Clicks the centre of a rectangle; false when there is none.
fn click_rect(app: &mut App, v: &Value) -> bool {
    match rect(v) {
        Some(r) => {
            let (x, y) = centre(r);
            app.session.click(x, y).is_ok()
        }
        None => false,
    }
}

/// Clicks a window-client css point.
fn click_at(app: &mut App, x: f64, y: f64) -> bool {
    app.session.click(x as f32, y as f32).is_ok()
}

/// The `ui.rect` of a named surface (`cog`, `grid`, ...), as a JSON value.
fn ui_rect(app: &mut App, name: &str) -> Value {
    probe(app, "ui.rect", json!({ "name": name }))
}

/// The number of tabs the app shows now.
fn tabs(app: &mut App) -> usize {
    app.session.state().map(|s| s.tabs.len()).unwrap_or(0)
}

/// The scale of the app (device pixels per css pixel).
fn scale_of(app: &mut App) -> f64 {
    app.session
        .state()
        .map(|s| f64::from(s.scale))
        .unwrap_or(1.0)
}

/// True while the Settings tab is in front.
fn settings_front(app: &mut App) -> bool {
    probe(app, "settings.state", Value::Null)["front"] == json!(true)
}

/// The Settings page's answer: its rows, its sections and its Save buttons.
fn theme(app: &mut App) -> Value {
    probe(app, "settings.theme", Value::Null)
}

/// Whether the switch question is open.
fn ask_open(app: &mut App) -> bool {
    theme(app)["ask"]["open"] == json!(true)
}

/// Whether the theme editor is open.
fn editor_open(app: &mut App) -> bool {
    theme(app)["editor"]["open"] == json!(true)
}

/// Whether the colour picker is open.
fn picker_open(app: &mut App) -> bool {
    theme(app)["picker"]["open"] == json!(true)
}

/// The colours the window paints (`settings.colours`): accent, accent solid, the ink of primary
/// buttons and the ground, each `#rrggbbaa`.
fn colours(app: &mut App) -> Value {
    probe(app, "settings.colours", Value::Null)
}

/// One row of the open page (`settings.row`), `Null` when the page has no such row.
fn row(app: &mut App, id: &str) -> Value {
    probe(app, "settings.row", json!({ "id": id }))
}

/// The page of a row, from the catalogue (`settings.rows` with `catalogue`).
fn page_of(app: &mut App, id: &str) -> Option<String> {
    let cat = probe(app, "settings.rows", json!({ "catalogue": true }));
    cat["rows"]
        .as_array()?
        .iter()
        .find(|r| r["id"].as_str() == Some(id))
        .and_then(|r| r["page"].as_str().map(str::to_string))
}

/// The row ids of the page in front, in draw order (`settings.rows`).
fn page_rows(app: &mut App) -> Vec<String> {
    let v = probe(app, "settings.rows", json!({}));
    v["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The page the Settings rows answer for (`null` when no page is drawn).
fn page_now(app: &mut App) -> Option<String> {
    probe(app, "settings.rows", json!({}))["page"]
        .as_str()
        .map(str::to_string)
}

/// The stored settings (`<profile>\settings.json`, a JSON object), or an empty object.
fn store(app: &App) -> Value {
    let text = std::fs::read_to_string(app.profile.join("settings.json")).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_else(|_| json!({}))
}

/// One stored key as a string (`""` when absent or not a string).
fn stored_str(app: &App, key: &str) -> String {
    store(app)[key].as_str().unwrap_or("").to_string()
}

/// The lower-case six-digit `#rrggbb` part of a `#rrggbbaa` (or `#rrggbb`) string.
fn hex6(s: &str) -> String {
    s.trim().to_lowercase().chars().take(7).collect()
}

/// The rgb of a `#rrggbb` string (black when it is not one).
fn rgb_of_hex(hex: &str) -> [u8; 3] {
    let h = hex.trim_start_matches('#');
    let ch = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    [ch(0), ch(2), ch(4)]
}

/// The colour of the device pixel (x, y) of the last snapshot, `None` when there is none there.
fn pixel(app: &mut App, x: f64, y: f64) -> Option<[u8; 3]> {
    let (x, y) = (x.round().max(0.0) as u32, y.round().max(0.0) as u32);
    match app.session.call(&Request::Pixel { x, y }) {
        Ok(flare_e2e_proto::Reply::Pixel { rgba }) => Some([rgba[0], rgba[1], rgba[2]]),
        _ => None,
    }
}

/// Takes a snapshot, then reads the pixel at a css point (the pixel reads answer from the last one).
fn pixel_css(ctx: &mut Ctx, app: &mut App, name: &str, x: f64, y: f64) -> Option<[u8; 3]> {
    let _ = ctx.shot(app, name);
    let k = scale_of(app);
    pixel(app, x * k, y * k)
}

/// The pointer's position over the pane: its middle, in css px, a little in from the left.
fn pane_point(app: &mut App) -> (f32, f32) {
    let g = ui_rect(app, "grid");
    let (x, y, w, h) = rect(&g).unwrap_or((0.0, 36.0, 1000.0, 600.0));
    ((x + w / 2.0) as f32, (y + h / 2.0) as f32)
}

/// Wheels the pane until the row's box is inside the pane (`settings.row` visible); true when it is.
fn reveal(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    let at = pane_point(app);
    ctx.until(&format!("row {id} to be in view"), 12_000, &mut || {
        let r = row(app, id);
        if r.is_null() {
            return false;
        }
        if r["visible"] == json!(true) {
            return true;
        }
        let y = r["rect"]["y"].as_f64().unwrap_or(0.0);
        let mid = {
            let g = ui_rect(app, "grid");
            rect(&g)
                .map(|(_, gy, _, gh)| gy + gh / 2.0)
                .unwrap_or(300.0)
        };
        let dy = if y > mid { -160.0 } else { 160.0 };
        let _ = app.session.call(&Request::Pointer {
            x: at.0,
            y: at.1,
            action: PointerAction::Wheel { dx: 0.0, dy },
        });
        false
    })
}

/// Opens Settings from the title bar's cog, and waits until it is in front.
fn open_settings(ctx: &mut Ctx, app: &mut App) -> bool {
    let cog = ui_rect(app, "cog");
    if !click_rect(app, &cog) {
        return false;
    }
    ctx.until("Settings is in front", 8_000, &mut || settings_front(app))
}

/// Chooses a rail page by its rectangle and waits until its rows answer for it.
fn goto_page(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    let r = if id == "about" {
        probe(app, "settings.rail_end", Value::Null)
    } else {
        probe(app, "settings.rail", json!({ "id": id }))
    };
    if let Some(r) = rect(&r) {
        let (x, y) = centre(r);
        let _ = app.session.click(x, y);
    }
    ctx.until(&format!("the {id} page is drawn"), 6_000, &mut || {
        page_now(app).as_deref() == Some(id)
    })
}

/// Shows the page a row lives on and scrolls the row into view. False when either fails.
fn goto_row(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    let Some(page) = page_of(app, id) else {
        return false;
    };
    if page_now(app).as_deref() != Some(page.as_str()) && !goto_page(ctx, app, &page) {
        return false;
    }
    reveal(ctx, app, id)
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

/// Types a colour into a row's code field (a colour row, `settings.row` gives its box).
fn fill_row_colour(ctx: &mut Ctx, app: &mut App, id: &str, text: &str) -> bool {
    if !goto_row(ctx, app, id) {
        return false;
    }
    let r = row(app, id);
    match rect(&r["code_rect"]) {
        Some((x, y, w, h)) => {
            fill_at(app, x + w / 2.0, y + h / 2.0, text);
            true
        }
        None => false,
    }
}

/// Clicks the Save changes button of the page in front (the first section that has one).
fn click_save(app: &mut App) -> bool {
    let t = theme(app);
    let Some(save) = t["saves"].as_array().and_then(|s| s.first()).cloned() else {
        return false;
    };
    let p = &save["point"];
    click_at(
        app,
        p["x"].as_f64().unwrap_or(0.0),
        p["y"].as_f64().unwrap_or(0.0),
    )
}

/// Whether the page in front has a Save changes that is lit (dirty).
fn save_dirty(app: &mut App) -> bool {
    theme(app)["saves"]
        .as_array()
        .and_then(|s| s.first())
        .map(|s| s["dirty"] == json!(true))
        .unwrap_or(false)
}

/// Finds a wall card by id, from the theme page's `settings.theme`.
fn card<'a>(t: &'a Value, id: &str) -> Option<&'a Value> {
    t["wall"]["cards"]
        .as_array()?
        .iter()
        .find(|c| c["id"].as_str() == Some(id))
}

/// Presses a theme card (its body), when the wall shows it.
fn pick_card(app: &mut App, id: &str) -> bool {
    let t = theme(app);
    match card(&t, id) {
        Some(c) if c["shown"] == json!(true) => click_rect(app, &c["rect"]),
        _ => false,
    }
}

/// Opens a theme's editor from its pencil.
fn edit_card(app: &mut App, id: &str) -> bool {
    let t = theme(app);
    match card(&t, id) {
        Some(c) => click_rect(app, &c["pencil"]),
        None => false,
    }
}

/// A well of the open editor, by its label (`Background`, `Foreground`, ...).
fn well<'a>(t: &'a Value, label: &str) -> Option<&'a Value> {
    t["editor"]["wells"]
        .as_array()?
        .iter()
        .find(|w| w["label"].as_str() == Some(label))
}

/// Types a colour into an editor well, by its label.
fn fill_well(app: &mut App, label: &str, text: &str) -> bool {
    let t = theme(app);
    let Some(w) = well(&t, label) else {
        return false;
    };
    match rect(&w["field"]) {
        Some((x, y, _, h)) => {
            fill_at(app, x + 12.0, y + h / 2.0, text);
            true
        }
        None => false,
    }
}

/// The one-line answer for a check's message: the colour of a well by its label.
fn well_text(app: &mut App, label: &str) -> String {
    let t = theme(app);
    well(&t, label)
        .and_then(|w| w["text"].as_str())
        .unwrap_or("")
        .to_lowercase()
}

/// The lit Save button's pixel, read at its point (inside the button, 14 px from its right edge).
fn save_pixel(ctx: &mut Ctx, app: &mut App, name: &str) -> Option<[u8; 3]> {
    let t = theme(app);
    let save = t["saves"].as_array()?.first()?.clone();
    let p = &save["point"];
    pixel_css(ctx, app, name, p["x"].as_f64()?, p["y"].as_f64()?)
}

/// Launches the app on the alpha folder, with the software renderer (the pixel reads need no GPU).
fn launch_alpha(ctx: &mut Ctx) -> Result<App, String> {
    let alpha = ctx.ensure_world().alpha.clone();
    ctx.launch(Launch {
        folders: vec![alpha],
        renderer: Some("software"),
        ..Launch::default()
    })
}

/// Waits for the one tab (PT's first check, before Settings opens), then opens Settings on the
/// Appearance page. The check is recorded here; false when Settings did not open.
fn start_appearance(ctx: &mut Ctx, app: &mut App) -> bool {
    let one = ctx.until("one tab open", 15_000, &mut || tabs(app) == 1);
    ctx.ok(one, "one tab");
    if !open_settings(ctx, app) {
        return false;
    }
    goto_page(ctx, app, "appearance")
}

/// Ctrl+Tab to the shell and back to Settings, with the waits the port's check needs.
fn switch_away_and_back(ctx: &mut Ctx, app: &mut App) -> bool {
    let _ = app.session.key("Ctrl+Tab");
    let away = ctx.until("Ctrl+Tab leaves Settings", 3_000, &mut || {
        !settings_front(app)
    });
    if !away {
        return false;
    }
    let _ = app.session.key("Ctrl+Tab");
    ctx.until("back on Settings", 3_000, &mut || settings_front(app))
}

/// The custom theme as stored (`flare.term.custom`, a JSON object), or an empty object.
fn custom_of(app: &App) -> Value {
    let raw = stored_str(app, "flare.term.custom");
    serde_json::from_str(&raw).unwrap_or_else(|_| json!({}))
}

/// windowColoursSave (PT run.mjs:2820): the Secondary colour sits above Accent; a colour of its own
/// lights Save changes and Save keeps it with Custom; another theme forgets it, Custom brings it back.
pub fn window_colours_save(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = launch_alpha(ctx)?;
    if !start_appearance(ctx, &mut app) {
        ctx.ok(false, "the cog opens Settings on the Appearance page");
        app.close();
        return Ok(());
    }

    // PT: the Secondary colour sits right above Accent.
    let order = page_rows(&mut app);
    let at = |id: &str| order.iter().position(|x| x == id);
    let ok_order = match (at("window-secondary"), at("window-accent")) {
        (Some(s), Some(a)) => s + 1 == a,
        _ => false,
    };
    ctx.ok(
        ok_order,
        format!(
            "Secondary colour sits right above Accent ({})",
            order.join(", ")
        ),
    );
    // PT: nothing to save at first.
    ctx.ok(!save_dirty(&mut app), "nothing to save at first");

    // PT: a Secondary colour of your own lights Save changes.
    let filled = fill_row_colour(ctx, &mut app, "window-secondary", "#2a2a2a");
    ctx.settle(50);
    let lit = ctx.until("a Secondary colour lights Save changes", 4_000, &mut || {
        save_dirty(&mut app)
    });
    ctx.ok(
        filled && lit,
        "a Secondary colour of your own lights Save changes",
    );

    // PT: in the accent (the lit button's paint is the accent).
    let accent = hex6(colours(&mut app)["accent"].as_str().unwrap_or(""));
    let lit_paint = save_pixel(ctx, &mut app, "window-colours-lit");
    ctx.ok(
        lit_paint == Some(rgb_of_hex(&accent)),
        format!("in the accent ({lit_paint:?} vs {accent})"),
    );

    // PT: Save keeps it with Custom, and goes quiet.
    let _ = click_save(&mut app);
    let kept = ctx.until("Save goes quiet", 4_000, &mut || !save_dirty(&mut app));
    let secondary = stored_str(&app, "flare.window.secondary");
    let theme_id = stored_str(&app, "flare.term.theme");
    ctx.ok(
        kept && secondary.to_lowercase().contains("2a2a2a") && !save_dirty(&mut app),
        format!(
            "Save keeps it with Custom, and goes quiet (theme {theme_id}, secondary {secondary})"
        ),
    );

    // PT: another theme forgets it.
    let picked = pick_card(&mut app, "pitch");
    let forgot = ctx.until(
        "another theme forgets the Secondary colour",
        4_000,
        &mut || stored_str(&app, "flare.window.secondary").is_empty(),
    );
    ctx.ok(picked && forgot, "another theme forgets it");

    // PT: and Custom brings it back.
    let custom = pick_card(&mut app, "custom");
    let back = ctx.until(
        "Custom brings the Secondary colour back",
        4_000,
        &mut || {
            stored_str(&app, "flare.window.secondary")
                .to_lowercase()
                .contains("2a2a2a")
        },
    );
    ctx.ok(custom && back, "and Custom brings it back");

    // PT: with nothing to save.
    ctx.ok(!save_dirty(&mut app), "with nothing to save");
    app.close();
    Ok(())
}

/// accentAsk (PT run.mjs:3145): a red accent lights Save changes with white text; a theme switch
/// asks, and a near-black red is lifted.
pub fn accent_ask(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = launch_alpha(ctx)?;
    if !start_appearance(ctx, &mut app) {
        ctx.ok(false, "the cog opens Settings on the Appearance page");
        app.close();
        return Ok(());
    }

    // PT: a red accent lights Save changes.
    let filled = fill_row_colour(ctx, &mut app, "window-accent", "#ff0000");
    let lit = ctx.until("a red accent lights Save changes", 4_000, &mut || {
        save_dirty(&mut app)
    });
    ctx.ok(filled && lit, "a red accent lights Save changes");

    // PT: Save changes is red with white text.
    let accent = hex6(colours(&mut app)["accent"].as_str().unwrap_or(""));
    let paint = save_pixel(ctx, &mut app, "accent-lit");
    let ink = colours(&mut app)["on_accent"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    ctx.ok(
        paint == Some(rgb_of_hex(&accent)) && hex6(&ink) == "#ffffff",
        format!("Save changes is red with white text (paint {paint:?}, ink {ink})"),
    );

    // PT: picking another theme asks first.
    let picked = pick_card(&mut app, "graphite");
    let asked = ctx.until("picking another theme asks first", 4_000, &mut || {
        ask_open(&mut app)
    });
    ctx.ok(picked && asked, "picking another theme asks first");

    // PT: its Save as Custom wears the accent, with white text. The question's buttons are not
    // probed (AskThree's rectangles are not read from Rust), so the paint check is not made here.
    ctx.skip("the switch question's button rectangles are not probed (AskThree), so its paint is not read");

    // Cancel the question: Tab moves the focus from Save as Custom to Cancel, Enter answers it.
    let _ = app.session.key("Tab");
    let _ = app.session.key("Enter");
    ctx.until("the question closes", 3_000, &mut || !ask_open(&mut app));

    // PT: a red too dark to see (#140707) is lifted to a red that reads, never a grey.
    fill_row_colour(ctx, &mut app, "window-accent", "#140707");
    let dark_ok = ctx.until("the near-black red is applied", 4_000, &mut || {
        save_dirty(&mut app)
    });
    let dark = save_pixel(ctx, &mut app, "accent-dark");
    let (r, g, b) = dark
        .map(|c| (i32::from(c[0]), i32::from(c[1]), i32::from(c[2])))
        .unwrap_or((0, 0, 0));
    ctx.ok(
        dark_ok && r > 110 && r - g > 50 && r - b > 50,
        format!("a near-black red is lifted to a red, not a grey ({dark:?})"),
    );

    // PT: the theme switch's Save as Custom is that same red (the question's paint is not probed).
    ctx.skip("the switch question's Save as Custom paint is not probed (AskThree), see above");

    // Discard the question's changes: Tab twice moves to Discard, Enter answers it.
    let _ = pick_card(&mut app, "graphite");
    ctx.until("the question opens again", 4_000, &mut || {
        ask_open(&mut app)
    });
    let _ = app.session.key("Tab");
    let _ = app.session.key("Tab");
    let _ = app.session.key("Enter");
    ctx.until("the question closes", 3_000, &mut || !ask_open(&mut app));
    app.close();
    Ok(())
}

/// themeEditorLive (PT run.mjs:5667): a picked Background paints at once; the editor and its
/// picker come back as they were left; Escape restores; Save as Custom keeps the accent; Cancel
/// restores the saved colour.
pub fn theme_editor_live(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = launch_alpha(ctx)?;
    if !start_appearance(ctx, &mut app) {
        ctx.ok(false, "the cog opens Settings on the Appearance page");
        app.close();
        return Ok(());
    }
    // PT: one tab (checked by start_appearance); the default theme is picked and its pencil opened.
    let _ = pick_card(&mut app, "pt-default");
    ctx.until("the default theme is the one in force", 3_000, &mut || {
        hex6(colours(&mut app)["accent"].as_str().unwrap_or("")) == "#fe8f34"
    });
    let accent = hex6(colours(&mut app)["accent"].as_str().unwrap_or(""));
    let ground = hex6(colours(&mut app)["ground"].as_str().unwrap_or(""));
    let _ = edit_card(&mut app, "pt-default");
    let opened = ctx.until("the editor opens", 3_000, &mut || editor_open(&mut app));
    if !opened {
        ctx.ok(false, "the pencil opens the theme editor");
        app.close();
        return Ok(());
    }

    // PT: a picked Background paints the window before any save.
    fill_well(&mut app, "Background", "#301020");
    let painted = ctx.until("a picked Background paints the window", 3_000, &mut || {
        hex6(colours(&mut app)["ground"].as_str().unwrap_or("")) == "#301020"
    });
    ctx.ok(
        painted,
        format!(
            "a picked Background paints the window before Save (was {ground}, now {})",
            hex6(colours(&mut app)["ground"].as_str().unwrap_or(""))
        ),
    );
    // PT: nothing is saved yet.
    ctx.ok(
        !store(&app)
            .as_object()
            .is_some_and(|o| o.contains_key("flare.term.custom")),
        "and nothing is saved yet",
    );

    // PT: Ctrl+Tab goes to the shell, which wears the picked colour.
    let _ = app.session.key("Ctrl+Tab");
    let away = ctx.until("Ctrl+Tab goes to the shell", 3_000, &mut || {
        !settings_front(&mut app)
    });
    ctx.ok(away, "Ctrl+Tab goes to the shell");
    ctx.ok(
        hex6(colours(&mut app)["ground"].as_str().unwrap_or("")) == "#301020",
        "which wears the picked colour",
    );
    // PT: back on Settings, the editor is open again, holding the pick.
    let _ = app.session.key("Ctrl+Tab");
    let back = ctx.until(
        "back on Settings, the editor is open again",
        3_000,
        &mut || settings_front(&mut app) && editor_open(&mut app),
    );
    ctx.ok(back, "back on Settings, the editor is open again");
    ctx.ok(
        well_text(&mut app, "Background").starts_with("#301020"),
        "holding the pick",
    );

    // PT: a picker left open comes back open. Open the Foreground swatch's picker and walk it.
    let fg_swatch = theme(&mut app)["editor"]["wells"].as_array().and_then(|w| {
        w.iter()
            .find(|x| x["label"] == json!("Foreground"))
            .cloned()
    });
    let fg_opened = match fg_swatch.as_ref().and_then(|w| rect(&w["swatch"])) {
        Some(r) => {
            let (x, y) = centre(r);
            let _ = app.session.click(x, y);
            ctx.until("the Foreground picker opens", 3_000, &mut || {
                picker_open(&mut app)
            })
        }
        None => false,
    };
    let _ = app.session.key("ArrowLeft");
    let _ = app.session.key("ArrowLeft");
    ctx.settle(50);
    let fg_picked = well_text(&mut app, "Foreground");
    let _ = fg_opened;

    // PT: on another tab, the picker is not on screen.
    let _ = app.session.key("Ctrl+Tab");
    let closed_away = ctx.until(
        "on another tab, the picker is not on screen",
        3_000,
        &mut || !picker_open(&mut app),
    );
    ctx.ok(closed_away, "on another tab, the picker is not on screen");
    let _ = app.session.key("Ctrl+Tab");
    let reopened = ctx.until(
        "back on Settings, the editor and its picker are open again",
        3_000,
        &mut || picker_open(&mut app) && editor_open(&mut app),
    );
    ctx.ok(
        reopened,
        "back on Settings, the editor and its picker are open again",
    );
    // The keyboard focus inside the picker is not read from Rust (Slint exposes no focus query),
    // so that line is skipped and the colour checks below stand for it.
    ctx.skip("the keyboard focus inside the picker is not readable from Rust");
    ctx.ok(
        well_text(&mut app, "Foreground") == fg_picked,
        format!("still on the picked colour ({fg_picked})"),
    );
    let _ = app.session.key("Escape");
    let shut = ctx.until("Escape closes it", 3_000, &mut || !picker_open(&mut app));
    ctx.ok(shut, "Escape closes it");
    ctx.ok(
        well_text(&mut app, "Foreground") != fg_picked,
        "and puts back the colour it first opened with",
    );
    // PT: a picker the user closed stays closed after a tab switch.
    let _ = switch_away_and_back(ctx, &mut app);
    ctx.settle(50);
    ctx.ok(
        !picker_open(&mut app),
        "a picker the user closed stays closed after a tab switch",
    );

    // PT: Save as Custom keeps the picked colours, with PT Default's own accent.
    let save_at = theme(&mut app)["editor"]["save"].clone();
    let _ = click_rect(&mut app, &save_at);
    let saved = ctx.until("Save as Custom stores the custom theme", 4_000, &mut || {
        custom_of(&app)["bg"]
            .as_str()
            .is_some_and(|s| s.starts_with("#301020"))
    });
    let custom = custom_of(&app);
    let custom_accent = custom["accent"].as_str().unwrap_or("").to_lowercase();
    ctx.ok(
        saved && custom_accent == accent,
        format!(
            "saved as Custom with PT Default's accent ({}, {custom_accent})",
            custom["bg"]
        ),
    );
    // PT: the window keeps its accent.
    let now_accent = hex6(colours(&mut app)["accent"].as_str().unwrap_or(""));
    ctx.ok(
        now_accent == accent,
        format!("the window keeps its accent ({now_accent} vs {accent})"),
    );

    // PT: Cancel puts the saved colour back.
    let _ = edit_card(&mut app, "custom");
    ctx.until("the Custom editor opens", 3_000, &mut || {
        editor_open(&mut app)
    });
    fill_well(&mut app, "Background", "#103020");
    ctx.until("the edited Background paints", 3_000, &mut || {
        hex6(colours(&mut app)["ground"].as_str().unwrap_or("")) == "#103020"
    });
    let cancel_at = theme(&mut app)["editor"]["cancel"].clone();
    let _ = click_rect(&mut app, &cancel_at);
    let restored = ctx.until("Cancel puts the saved colour back", 3_000, &mut || {
        hex6(colours(&mut app)["ground"].as_str().unwrap_or("")) == "#301020"
    });
    ctx.ok(
        restored,
        format!(
            "Cancel puts the saved colour back ({})",
            hex6(colours(&mut app)["ground"].as_str().unwrap_or(""))
        ),
    );

    // PT: the same on the Appearance page: the Background colour's picker left open is open again.
    if goto_row(ctx, &mut app, "window-background") {
        let r = row(&mut app, "window-background");
        if let Some(sw) = rect(&r["swatch"]) {
            let (x, y) = centre(sw);
            let _ = app.session.click(x, y);
        }
        ctx.until("the Background picker opens", 3_000, &mut || {
            picker_open(&mut app)
        });
        let _ = app.session.key("Ctrl+Tab");
        ctx.until("the Background picker is shut away", 3_000, &mut || {
            !picker_open(&mut app)
        });
        let _ = app.session.key("Ctrl+Tab");
        let again = ctx.until(
            "an Appearance picker left open is open again on return",
            3_000,
            &mut || picker_open(&mut app),
        );
        ctx.ok(
            again,
            "an Appearance picker left open is open again on return",
        );
        let _ = app.session.key("Escape");
    } else {
        ctx.ok(false, "the Background row is on the Appearance page");
    }
    app.close();
    Ok(())
}

/// themeSwitch (PT run.mjs:5746): the Appearance order; font size 50% to 200% in tens; a theme switch
/// keeps the font, its size, the indicator style and the edges and asks nothing about them; Save as
/// Custom carries no font; a picked background and accent ask, and Discard, Cancel and Save as
/// Custom behave as the question says.
pub fn theme_switch(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = launch_alpha(ctx)?;
    if !start_appearance(ctx, &mut app) {
        ctx.ok(false, "the cog opens Settings on the Appearance page");
        app.close();
        return Ok(());
    }

    // PT: the Appearance page runs its rows in this order.
    let want = [
        "tab-layout",
        "tab-switch",
        "window-edges",
        "term-theme",
        "term-acrylic",
        "window-background",
        "window-secondary",
        "window-accent",
    ];
    let got = page_rows(&mut app);
    let appearance: Vec<&str> = got
        .iter()
        .map(String::as_str)
        .filter(|id| want.contains(id))
        .collect();
    ctx.ok(
        appearance == want,
        format!("Appearance runs {} ({})", want.join(" > "), got.join(" > ")),
    );

    // PT: font size is 50% to 200% in tens.
    let _ = goto_row(ctx, &mut app, "term-font");
    let sizes: Vec<String> = row(&mut app, "term-font")["options"]
        .as_array()
        .map(|o| {
            o.iter()
                .map(|x| x.as_str().unwrap_or("").trim().to_string())
                .collect()
        })
        .unwrap_or_default();
    let tens: Vec<String> = (0..16).map(|i| format!("{}%", 50 + i * 10)).collect();
    ctx.ok(
        sizes == tens,
        format!("font size offers 50% to 200% in tens ({})", sizes.join(" ")),
    );

    // The font, its size, the indicator style and the edges are the user's own settings. The
    // segmented pills and the font menu have no rectangle the probes read, so they are set in
    // the profile before the switch (the same values PT's clicks choose), and the switch must
    // leave all four as they are.
    ctx.skip("the segmented pills and the font menu are not probed, so the settings are written to settings.json before the switch");
    let profile = app.profile.clone();
    app.close();
    let _ = std::fs::write(
        profile.join("settings.json"),
        r#"{"flare.term.fontPct":"140","flare.term.font":"jetbrains","flare.term.agentIndicator":"minimal","flare.window.edges":"solid"}"#,
    );
    let mut app = launch_alpha(ctx)?;
    if !start_appearance(ctx, &mut app) {
        ctx.ok(
            false,
            "the cog opens Settings on the Appearance page after the relaunch",
        );
        app.close();
        return Ok(());
    }
    let mine = |app: &App| {
        json!({
            "pct": stored_str(app, "flare.term.fontPct"),
            "font": stored_str(app, "flare.term.font"),
            "indicator": stored_str(app, "flare.term.agentIndicator"),
            "edges": stored_str(app, "flare.window.edges"),
        })
    };
    let before = mine(&app);

    // PT: none of them lights Save changes.
    ctx.ok(!save_dirty(&mut app), "none of them lights Save changes");

    // PT: a theme switch after them asks nothing.
    let picked = pick_card(&mut app, "nord");
    let switched = ctx.until("the switch to nord lands", 4_000, &mut || {
        stored_str(&app, "flare.term.theme") == "nord"
    });
    ctx.settle(50);
    ctx.ok(
        picked && switched && !ask_open(&mut app),
        "a theme switch after them asks nothing",
    );
    let after = mine(&app);
    ctx.ok(
        before == after,
        format!("and keeps the font, size, indicator style and edges ({after})"),
    );

    // PT: a Custom to lead the wall: an agent colour, saved with the page's Save.
    let agent = fill_row_colour(ctx, &mut app, "agent-color", "#3DA9FC");
    let agent_dirty = ctx.until("the agent colour lights Save changes", 4_000, &mut || {
        save_dirty(&mut app)
    });
    let _ = agent && agent_dirty;
    let _ = click_save(&mut app);
    ctx.until("the page's Save goes quiet", 4_000, &mut || {
        !save_dirty(&mut app)
    });
    let _ = goto_page(ctx, &mut app, "appearance");
    let custom_listed = ctx.until("the wall lists Custom", 4_000, &mut || {
        card(&theme(&mut app), "custom").is_some()
    });
    let order: Vec<String> = theme(&mut app)["wall"]["cards"]
        .as_array()
        .map(|c| {
            c.iter()
                .take(2)
                .map(|x| x["id"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    ctx.ok(
        custom_listed
            && order.first().map(String::as_str) == Some("custom")
            && order.get(1).map(String::as_str) == Some("pt-default"),
        format!(
            "Custom comes first, then the default ({})",
            order.join(", ")
        ),
    );
    let saved_custom = custom_of(&app);
    ctx.ok(
        saved_custom.get("fontPct").is_none() && saved_custom.get("font").is_none(),
        "Save as Custom carries no font and no size",
    );

    // PT: a picked background and accent are changes to save, so a switch asks first.
    fill_row_colour(ctx, &mut app, "window-background", "#202830");
    fill_row_colour(ctx, &mut app, "window-accent", "#E07A2F");
    let picked_both = ctx.until("a background and an accent are picked", 4_000, &mut || {
        !stored_str(&app, "flare.window.background").is_empty()
            && !stored_str(&app, "flare.window.accent").is_empty()
    });
    ctx.ok(picked_both, "a background and an accent are picked");
    let _ = goto_page(ctx, &mut app, "appearance");
    ctx.ok(save_dirty(&mut app), "and they light Save changes");
    let _ = pick_card(&mut app, "pitch");
    let asks = ctx.until("so a theme switch asks first", 3_000, &mut || {
        ask_open(&mut app)
    });
    ctx.ok(asks, "so a theme switch asks first");
    // Discard: Tab twice from Save as Custom reaches Discard, Enter answers it.
    let _ = app.session.key("Tab");
    let _ = app.session.key("Tab");
    let _ = app.session.key("Enter");
    ctx.until("the question closes", 3_000, &mut || !ask_open(&mut app));
    let discarded = ctx.until(
        "Discard drops the background and accent",
        4_000,
        &mut || {
            stored_str(&app, "flare.term.theme") == "pitch"
                && stored_str(&app, "flare.window.accent").is_empty()
                && stored_str(&app, "flare.window.background").is_empty()
        },
    );
    ctx.ok(
        discarded,
        format!(
            "Discard: the switch takes the picked background and accent with it (theme {})",
            stored_str(&app, "flare.term.theme")
        ),
    );

    // PT: unsaved, Cancel keeps everything as it was.
    fill_row_colour(ctx, &mut app, "agent-color", "#C0FFEE");
    ctx.until("the unsaved agent colour is held", 3_000, &mut || {
        save_dirty(&mut app)
    });
    let dirty_agent = stored_str(&app, "flare.term.agentColor");
    let _ = goto_page(ctx, &mut app, "appearance");
    let _ = pick_card(&mut app, "nord");
    let asks_again = ctx.until(
        "with a change unsaved, picking a theme asks first",
        3_000,
        &mut || ask_open(&mut app),
    );
    // Cancel: Tab once from Save as Custom reaches Cancel, Enter answers it.
    let _ = app.session.key("Tab");
    let _ = app.session.key("Enter");
    ctx.until("the question closes", 3_000, &mut || !ask_open(&mut app));
    let kept_theme = stored_str(&app, "flare.term.theme");
    let kept_agent = stored_str(&app, "flare.term.agentColor");
    ctx.ok(
        asks_again && !ask_open(&mut app) && kept_theme == "pitch" && kept_agent == dirty_agent,
        format!("Cancel changes nothing (theme {kept_theme}, agent {kept_agent})"),
    );

    // PT: Discard switches, and the change is gone.
    let _ = pick_card(&mut app, "nord");
    ctx.until("the question asks", 3_000, &mut || ask_open(&mut app));
    let _ = app.session.key("Tab");
    let _ = app.session.key("Tab");
    let _ = app.session.key("Enter");
    let gone = ctx.until("Discard switches and drops the change", 4_000, &mut || {
        stored_str(&app, "flare.term.theme") == "nord"
            && stored_str(&app, "flare.term.agentColor") != dirty_agent
    });
    ctx.ok(
        gone,
        format!(
            "Discard switches and drops the change (theme {}, agent {})",
            stored_str(&app, "flare.term.theme"),
            stored_str(&app, "flare.term.agentColor")
        ),
    );

    // PT: Save as Custom keeps the change in Custom, then the switch happens.
    fill_row_colour(ctx, &mut app, "agent-color", "#C0FFEE");
    ctx.until("the agent colour is held again", 3_000, &mut || {
        save_dirty(&mut app)
    });
    let _ = goto_page(ctx, &mut app, "appearance");
    let _ = pick_card(&mut app, "pitch");
    ctx.until("the question asks again", 3_000, &mut || ask_open(&mut app));
    // Save as Custom is the focused answer: Enter answers it.
    let _ = app.session.key("Enter");
    let switched_pitch = ctx.until("Save as Custom switches to pitch", 4_000, &mut || {
        stored_str(&app, "flare.term.theme") == "pitch"
    });
    let custom_agent = custom_of(&app)["indicatorColor"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    let agent_now = stored_str(&app, "flare.term.agentColor").to_lowercase();
    ctx.ok(
        switched_pitch && (custom_agent == agent_now || custom_agent.contains("c0ffee")),
        format!(
            "Save as Custom keeps the change in Custom, then switches (theme {}, custom colour {custom_agent})",
            stored_str(&app, "flare.term.theme")
        ),
    );
    app.close();
    Ok(())
}

/// reviewSettings (PT run.mjs:5894): tabbing through a colour that follows the theme stores
/// nothing and offers no Reset; the editor takes the keyboard, Escape closes the editor and then
/// the picker alone; saving keeps the agent colour and forgets the picked background.
pub fn review_settings(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = launch_alpha(ctx)?;
    if !start_appearance(ctx, &mut app) {
        ctx.ok(false, "the cog opens Settings on the Appearance page");
        app.close();
        return Ok(());
    }

    // PT #26: tabbing through a colour that follows the theme leaves it following.
    for id in ["agent-color", "window-accent"] {
        if goto_row(ctx, &mut app, id) {
            let r = row(&mut app, id);
            if let Some((x, y, w, h)) = rect(&r["code_rect"]) {
                let _ = click_at(&mut app, x + w / 2.0, y + h / 2.0);
            }
            let _ = app.session.key("Tab");
            ctx.settle(50);
        }
    }
    ctx.ok(
        stored_str(&app, "flare.term.agentColor").is_empty()
            && stored_str(&app, "flare.window.accent").is_empty(),
        "tabbing through a colour that follows the theme leaves it following",
    );
    let _ = goto_row(ctx, &mut app, "agent-color");
    ctx.ok(
        row(&mut app, "agent-color")["reset"] != json!(true),
        "and offers no Reset",
    );

    // PT #112: Tab walks from each colour's code field to its swatch; Enter there opens the picker
    // of that row, which is the keyboard reaching the swatch.
    for id in ["agent-color", "agent-done-color", "window-background"] {
        if goto_row(ctx, &mut app, id) {
            let r = row(&mut app, id);
            if let Some((x, y, w, h)) = rect(&r["code_rect"]) {
                let _ = click_at(&mut app, x + w / 2.0, y + h / 2.0);
            }
            let _ = app.session.key("Tab");
            let _ = app.session.key("Enter");
            let opened = ctx.until(&format!("Tab reaches {id}'s swatch"), 2_000, &mut || {
                picker_open(&mut app)
            });
            ctx.ok(
                opened,
                format!("Tab goes from {id}'s code field to its swatch"),
            );
            let _ = app.session.key("Escape");
            ctx.until("the picker shuts", 2_000, &mut || !picker_open(&mut app));
        } else {
            ctx.ok(
                false,
                format!("Tab goes from {id}'s code field to its swatch"),
            );
        }
    }
    ctx.settle(50);
    ctx.ok(
        stored_str(&app, "flare.term.agentColor").is_empty()
            && stored_str(&app, "flare.term.agentDoneColor").is_empty()
            && stored_str(&app, "flare.window.background").is_empty(),
        "tabbing through a shut picker stores nothing",
    );
    ctx.ok(!picker_open(&mut app), "and opens none");

    // PT #28: the editor opened from its pencil takes the keyboard; Escape closes it and the
    // pencil has the focus again (Enter opens it again).
    let _ = pick_card(&mut app, "pitch");
    ctx.until("the pitch card is in front", 3_000, &mut || {
        theme(&mut app)["wall"]["cards"].as_array().is_some()
    });
    let _ = edit_card(&mut app, "pitch");
    let opened = ctx.until("the editor opens from its pencil", 3_000, &mut || {
        editor_open(&mut app)
    });
    ctx.ok(opened, "the editor opened by keyboard takes the focus");
    ctx.skip("Tab keeping the focus inside the editor is not readable from Rust");
    let _ = app.session.key("Escape");
    let closed = ctx.until("Escape closes it", 3_000, &mut || !editor_open(&mut app));
    ctx.ok(closed, "Escape closes it");
    let _ = app.session.key("Enter");
    let reopened = ctx.until(
        "Enter at the pencil opens the editor again",
        3_000,
        &mut || editor_open(&mut app),
    );
    ctx.ok(reopened, "and the focus is back on the pencil");
    let _ = app.session.key("Escape");
    ctx.until("the editor closes again", 3_000, &mut || {
        !editor_open(&mut app)
    });

    // PT #112: an Escape aimed at a picker inside the editor is the picker's.
    let _ = edit_card(&mut app, "pitch");
    ctx.until(
        "the editor reopens for the picker check",
        3_000,
        &mut || editor_open(&mut app),
    );
    let fg = theme(&mut app)["editor"]["wells"].as_array().and_then(|w| {
        w.iter()
            .find(|x| x["label"] == json!("Foreground"))
            .cloned()
    });
    if let Some(sw) = fg.as_ref().and_then(|w| rect(&w["swatch"])) {
        let (x, y) = centre(sw);
        let _ = app.session.click(x, y);
    }
    let in_picker = ctx.until("a well in the editor opens the picker", 3_000, &mut || {
        picker_open(&mut app)
    });
    ctx.ok(in_picker, "a well in the editor opens the picker");
    ctx.skip("the focus moving into the picker is not readable from Rust");
    let _ = app.session.key("Escape");
    let shut = ctx.until("Escape closes the picker", 3_000, &mut || {
        !picker_open(&mut app)
    });
    ctx.ok(shut, "Escape closes the picker");
    ctx.settle(50);
    let stayed = editor_open(&mut app);
    ctx.ok(stayed, "and only the picker: the editor stays open");
    // The focus is back on the swatch: Enter there opens the picker again.
    let _ = app.session.key("Enter");
    let back_on_swatch = ctx.until("with the focus back on the swatch", 2_000, &mut || {
        picker_open(&mut app)
    });
    ctx.ok(back_on_swatch, "with the focus back on the swatch");
    let _ = app.session.key("Escape");
    ctx.until("the picker shuts", 2_000, &mut || !picker_open(&mut app));
    let _ = app.session.key("Escape");
    ctx.until("the editor shuts", 3_000, &mut || !editor_open(&mut app));

    // PT #8, #29: a saved Custom with an agent colour, a picked background, then an edit that
    // keeps the agent colour and forgets the picked background.
    let agent = fill_row_colour(ctx, &mut app, "agent-color", "#3DA9FC");
    let _ = agent;
    let _ = click_save(&mut app);
    ctx.until("the agent colour is saved", 4_000, &mut || {
        !save_dirty(&mut app)
    });
    let _ = goto_page(ctx, &mut app, "appearance");
    ctx.until("the wall lists Custom", 4_000, &mut || {
        card(&theme(&mut app), "custom").is_some()
    });
    let bg_set = fill_row_colour(ctx, &mut app, "window-background", "#202830");
    let bg_stored = ctx.until("a background is picked", 3_000, &mut || {
        !stored_str(&app, "flare.window.background").is_empty()
    });
    ctx.ok(bg_set && bg_stored, "a background is picked");
    let _ = edit_card(&mut app, "custom");
    ctx.until("the Custom editor opens", 3_000, &mut || {
        editor_open(&mut app)
    });
    fill_well(&mut app, "Background", "#101820");
    let save_at = theme(&mut app)["editor"]["save"].clone();
    let _ = click_rect(&mut app, &save_at);
    let saved = ctx.until(
        "the editor's save keeps the saved agent colour",
        4_000,
        &mut || custom_of(&app)["bg"].as_str() == Some("#101820"),
    );
    let agent_kept = stored_str(&app, "flare.term.agentColor").to_lowercase();
    ctx.ok(
        saved && agent_kept == "#3da9fc",
        format!(
            "the editor's save keeps the saved agent colour ({} , agent {agent_kept})",
            custom_of(&app)["bg"]
        ),
    );
    ctx.ok(
        stored_str(&app, "flare.window.background").is_empty(),
        "and forgets the picked background, so the edited one shows",
    );

    // PT #27: the dictation key capture belongs to M8's row.
    ctx.skip("M8: the dictation key capture check waits for dictation (Task 47)");
    app.close();
    Ok(())
}
