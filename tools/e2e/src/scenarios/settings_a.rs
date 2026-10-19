//! Settings A scenarios (Task 33): options, settingsLook, settingsSearch, settingsKeys. Each follows
//! its Prism Terminal line (PT: run.mjs, 5088, 5375, 5510, 5607); a body says where Flare differs.
//! Flare answers through the `settings.rows` and `settings.search` probes (the `SettingsBridge`
//! global), `settings.rail` (the rail's rectangle) and `ui.rect` (the title buttons).

use std::collections::BTreeSet;

use flare_e2e_proto::Request;
use serde_json::{json, Value};

use crate::ctx::{App, Ctx, Launch};

/// The six pages of the rail, in the rail's order.
const PAGES: [&str; 6] = [
    "appearance",
    "terminal",
    "agents",
    "dictation",
    "diagnostics",
    "about",
];

/// A probe's answer, `Null` when it fails (a check on it then reads false).
fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// A rectangle value (`x`, `y`, `w`, `h`) as a tuple, in css pixels.
fn boxed(v: &Value) -> Option<(f64, f64, f64, f64)> {
    Some((
        v["x"].as_f64()?,
        v["y"].as_f64()?,
        v["w"].as_f64()?,
        v["h"].as_f64()?,
    ))
}

/// A `ui.rect` answer for a named surface (`cog`, `grid`, ...), `None` when there is none.
fn rect(app: &mut App, name: &str) -> Option<(f64, f64, f64, f64)> {
    boxed(&probe(app, "ui.rect", json!({ "name": name })))
}

/// The centre of a css rectangle, as the pointer wants it.
fn centre(r: (f64, f64, f64, f64)) -> (f32, f32) {
    ((r.0 + r.2 / 2.0) as f32, (r.1 + r.3 / 2.0) as f32)
}

/// Clicks the centre of the named `ui.rect`; false when the app has no such rectangle.
fn click_rect(app: &mut App, name: &str) -> bool {
    match rect(app, name) {
        Some(r) => {
            let (x, y) = centre(r);
            app.session.click(x, y).is_ok()
        }
        None => false,
    }
}

/// The rectangle of a rail item, `None` for an unknown page. About is pinned to the rail's bottom,
/// so its box comes from `settings.rail_end`; the other pages come from `settings.rail`.
fn rail_rect(app: &mut App, id: &str) -> Option<(f64, f64, f64, f64)> {
    if id == "about" {
        return boxed(&probe(app, "settings.rail_end", Value::Null));
    }
    boxed(&probe(app, "settings.rail", json!({ "id": id })))
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

/// The rows the open Settings page draws (`settings.rows`).
fn rows_now(app: &mut App) -> Value {
    probe(app, "settings.rows", json!({}))
}

/// The page id the Settings rows answer for (`null` when no page is drawn).
fn page_now(app: &mut App) -> Option<String> {
    rows_now(app)["page"].as_str().map(str::to_string)
}

/// The find field's answer (`settings.search`).
fn search_now(app: &mut App) -> Value {
    probe(app, "settings.search", Value::Null)
}

/// Opens Settings from the title bar's cog, and waits until it is in front.
fn open_settings(ctx: &mut Ctx, app: &mut App) -> bool {
    if !click_rect(app, "cog") {
        return false;
    }
    ctx.until("Settings is in front", 8_000, &mut || settings_front(app))
}

/// Chooses a rail page by its rectangle and waits until the rows answer for it.
fn goto_page(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    if let Some(r) = rail_rect(app, id) {
        let (x, y) = centre(r);
        let _ = app.session.click(x, y);
    }
    // The About page draws its cards as blocks, not rows, so it has no row count to wait for.
    ctx.until(&format!("the {id} page is drawn"), 6_000, &mut || {
        page_now(app).as_deref() == Some(id)
            && (id == "about" || rows_now(app)["count"].as_u64().unwrap_or(0) > 0)
    })
}

/// The string at `key` of a JSON object, empty when there is none.
fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

/// True when `list` holds `id`.
fn has(list: &[String], id: &str) -> bool {
    list.iter().any(|x| x == id)
}

/// Sets the window's client size in css pixels (the e2e's `Resize`).
fn resize(app: &mut App, w: f32, h: f32) -> bool {
    app.session.call(&Request::Resize { w, h }).is_ok()
}

/// Gives the find field the keyboard, as PT's `find.focus()` does: a click in the field's box
/// (`settings.find_rect`). Where the field is folded (the narrow frame) there is no box to click,
/// and Ctrl+F is the terminal's find bar, not Settings', so the result is false.
fn focus_find(app: &mut App) -> bool {
    match boxed(&probe(app, "settings.find_rect", Value::Null)) {
        Some(r) => {
            // The window is never focused in the e2e (it may not take the foreground): the
            // window's own focus event is sent first, as a person returning to it would.
            let _ = app.session.call(&Request::Focus { on: true });
            let (x, y) = centre(r);
            app.session.click(x, y).is_ok()
        }
        None => false,
    }
}

/// The Settings test starts with one tab open on the alpha folder (PT: `args: [w.alpha]`).
fn launch_with_tab(ctx: &mut Ctx) -> Result<App, String> {
    let alpha = ctx.ensure_world().alpha.clone();
    ctx.launch(Launch {
        folders: vec![alpha],
        renderer: Some("software"),
        ..Launch::default()
    })
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

/// PT: run.mjs:5088-5230. Every terminal option is on the page, each core section is drawn once in
/// one panel in the list's order, this app's own rows are listed and drawn, and nothing else claims
/// to be a setting. The layout checks run on the rows' heights and the wall's rows; the Flare bridge
/// does not carry the icon tile, the panel's corner, the rule or the padding (skipped, with why).
pub fn options(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = launch_with_tab(ctx)?;
    ctx.until("one tab open", 15_000, &mut || tabs(&mut app) == 1);
    if !open_settings(ctx, &mut app) {
        ctx.ok(false, "the cog opens Settings");
        app.close();
        return Ok(());
    }
    // The catalogue of rows, read as data (PT reads the option tables as text).
    let cat = probe(&mut app, "settings.rows", json!({ "catalogue": true }));
    let all: Vec<Value> = cat["rows"].as_array().cloned().unwrap_or_default();
    let core: Vec<Value> = all
        .iter()
        .filter(|r| str_of(r, "source") != "app")
        .cloned()
        .collect();
    let wanted: Vec<String> = core.iter().map(|r| str_of(r, "id").to_string()).collect();
    ctx.ok(
        wanted.len() >= 21 && has(&wanted, "help-enabled") && has(&wanted, "diag-verbose"),
        format!(
            "the core lists the terminal, dictation, help and diagnostics options ({})",
            wanted.len()
        ),
    );
    ctx.ok(
        core.iter().all(|r| !str_of(r, "section").is_empty()),
        "and every entry names its section",
    );

    // Every page, in turn: every id drawn (rows and blocks), the rows drawn as settings, the
    // sections, and the page answers for the layout checks.
    let mut shown: BTreeSet<String> = BTreeSet::new();
    let mut shown_rows: BTreeSet<String> = BTreeSet::new();
    let mut sections: Vec<Value> = Vec::new();
    let mut per_page: Vec<(&str, Value)> = Vec::new();
    for tab in PAGES {
        goto_page(ctx, &mut app, tab);
        let v = rows_now(&mut app);
        for r in v["rows"].as_array().cloned().unwrap_or_default() {
            let id = str_of(&r, "id").to_string();
            if str_of(&r, "block") == "row" {
                shown_rows.insert(id.clone());
            }
            shown.insert(id);
        }
        sections.extend(v["sections"].as_array().cloned().unwrap_or_default());
        per_page.push((tab, v));
    }

    // The Dictation rows are M8's: the page draws no dictation row until M8 (Task 47), so they are
    // skipped here and not counted as missing.
    let (dictation, terminal): (Vec<&Value>, Vec<&Value>) = core
        .iter()
        .partition(|r| str_of(r, "source") == "dictation");
    if !dictation.is_empty() {
        ctx.skip(&format!(
            "the {} Dictation rows are M8's: the page draws none until M8 (Task 47)",
            dictation.len()
        ));
    }
    let wanted: Vec<String> = terminal
        .iter()
        .map(|r| str_of(r, "id").to_string())
        .collect();
    let missing: Vec<&String> = wanted.iter().filter(|id| !shown.contains(*id)).collect();
    ctx.ok(
        missing.is_empty(),
        format!("every terminal option is on the page (missing: {missing:?})"),
    );
    // ONE ORDER IN BOTH APPS, per section: each core section is drawn once, is one panel, and its
    // rows come in the list's own order. The Dictation sections are M8's (skipped above).
    let mut core_sections: Vec<String> = Vec::new();
    for r in &terminal {
        let s = str_of(r, "section").to_string();
        if !core_sections.contains(&s) {
            core_sections.push(s);
        }
    }
    for s in &core_sections {
        let list: Vec<String> = terminal
            .iter()
            .filter(|r| str_of(r, "section") == s)
            .map(|r| str_of(r, "id").to_string())
            .collect();
        let drawn: Vec<&Value> = sections.iter().filter(|x| str_of(x, "id") == s).collect();
        ctx.ok(
            drawn.len() == 1,
            format!("the core section {s} is drawn once ({})", drawn.len()),
        );
        if drawn.len() != 1 {
            continue;
        }
        let panels = drawn[0]["panels"].as_u64().unwrap_or(0);
        ctx.ok(panels == 1, format!("and is one panel ({panels})"));
        let order: Vec<String> = drawn[0]["ids"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|v| v.as_str().unwrap_or("").to_string())
            .filter(|id| list.contains(id))
            .collect();
        ctx.ok(
            order == list,
            format!(
                "{s}: its rows come in the shared order ({})",
                order.join(" > ")
            ),
        );
    }

    // What is left must be THIS APP's rows, a closed list: nothing else claims to be a setting.
    let own: Vec<String> = all
        .iter()
        .filter(|r| str_of(r, "source") == "app")
        .map(|r| str_of(r, "id").to_string())
        .collect();
    ctx.ok(
        own.len() >= 9 && has(&own, "window-edges") && has(&own, "taskbar-badge"),
        format!("this app's own rows are listed ({})", own.len()),
    );
    let extra: Vec<&String> = shown_rows
        .iter()
        .filter(|id| !wanted.contains(id) && !own.contains(id))
        .collect();
    ctx.ok(
        extra.is_empty(),
        format!("and nothing else claims to be a setting (extra: {extra:?})"),
    );
    let own_missing: Vec<&String> = own.iter().filter(|id| !shown.contains(*id)).collect();
    ctx.ok(
        own_missing.is_empty(),
        format!("and every one of them is drawn ({own_missing:?})"),
    );
    ctx.ok(
        !shown.contains("confirm-close"),
        "the close question is not a setting any more",
    );
    ctx.ok(
        !shown.contains("dictation-gpu"),
        "the GPU row is not offered without an NVIDIA card",
    );

    // The layout. The dictation model manager's rows are placeholders until M8.
    ctx.skip("the model manager is laid out: its rows are placeholders until M8 (Task 47)");
    // Every row is at least 58px tall. The Flare bridge carries a row's height only: its padding
    // (PT's 8px) is not probed, so only the height is checked here.
    let heights: Vec<f64> = per_page
        .iter()
        .flat_map(|(_, v)| v["rows"].as_array().cloned().unwrap_or_default())
        .filter(|r| str_of(r, "block") == "row")
        .filter_map(|r| r["h"].as_f64())
        .collect();
    let min_h = heights.iter().copied().fold(f64::INFINITY, f64::min);
    ctx.ok(
        !heights.is_empty() && min_h >= 58.0,
        format!("every row is at least 58px tall ({min_h:.1}px)"),
    );
    // The theme wall wraps into rows.
    let appearance = per_page
        .iter()
        .find(|(t, _)| *t == "appearance")
        .map(|(_, v)| v.clone())
        .unwrap_or(Value::Null);
    let wall_rows = appearance["wall"]["rows"].as_u64().unwrap_or(0);
    ctx.ok(
        wall_rows >= 2,
        format!("and the wall wraps into rows ({wall_rows})"),
    );
    ctx.skip("a theme card's width is not carried by the bridge (WallCard holds x and y only)");
    ctx.skip("the icon tile is 32px: the tile's size is not probed");
    ctx.skip("a panel's corner is the radius plus 3px: the panel's radius is not probed");
    ctx.skip("the hairline starts after the icon column: the rule's left edge is not probed");
    ctx.skip("no two controls in a row overlap: control boxes are not probed");

    app.close();
    Ok(())
}

/// PT: run.mjs:5375-5509. The Settings look at 1600 px and at 900 and 700 px wide. Flare measures
/// what the probes carry: the accent is not the chosen rail page's fill, the rail has its names at
/// 900 px and is icons under 760 px, and the page names are hidden there. The contrast of the label
/// and subtext, the accent-filled buttons, the sideways scroll, the Save lights, the acrylic check
/// and the magnifier are not measured (see the skips); the light theme is not picked here.
pub fn settings_look(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = launch_with_tab(ctx)?;
    ctx.until("one tab open", 15_000, &mut || tabs(&mut app) == 1);
    resize(&mut app, 1600.0, 1000.0);
    if !open_settings(ctx, &mut app) {
        ctx.ok(false, "the cog opens Settings");
        app.close();
        return Ok(());
    }
    ctx.skip("label and subtext contrast: the row's box inside the pane is known, but not the pane's origin in the window, so the ink cannot be located");
    ctx.skip("an icon reads 3:1 on its tile: the tile is not located (see the label check)");
    ctx.skip("a warning subtext reads 4.5:1: the dictation warning belongs to M8");
    ctx.skip("the chosen rail page is the hover fill: the hover colour is not exported");

    // The accent, from the start screen's mark (the same accent the window draws in).
    let accent = rgb_of_hex(str_of(
        &probe(&mut app, "start.state", Value::Null),
        "mark_colour",
    ));
    let k = scale_of(&mut app);
    for tab in PAGES {
        goto_page(ctx, &mut app, tab);
        let _ = ctx.shot(&mut app, &format!("settings-look-{tab}"));
        // The chosen rail item's fill, read in its right-hand part (no label or icon there).
        let fill = rail_rect(&mut app, tab)
            .and_then(|r| pixel(&mut app, (r.0 + r.2 - 6.0) * k, (r.1 + r.3 / 2.0) * k));
        ctx.ok(
            fill.is_some_and(|c| c != accent),
            format!("{tab}: and not the accent ({fill:?})"),
        );
    }
    ctx.skip("only Save changes and on switches wear the accent: the accent-filled buttons are not probed");
    ctx.skip("nothing scrolls sideways at 1600px: the pane's sideways scroll is not probed");

    // Lit Save changes, the acrylic check and the theme's Save are the skips below: no Save button
    // is probed, and the see-through check needs the GPU renderer (software stays solid, DIFF-5).
    ctx.skip("an agent colour of your own lights Save changes: no Save button is probed");
    ctx.skip("and the theme's Save changes with it: no Save button is probed");
    ctx.skip("with acrylic on, label and subtext still read: the see-through window is GPU-only (DIFF-5)");

    // 900 px: still the full rail, with its names.
    resize(&mut app, 900.0, 800.0);
    ctx.until("the rail is drawn at 900px", 3_000, &mut || {
        rail_rect(&mut app, "appearance").is_some_and(|r| r.2 > 0.0)
    });
    ctx.skip("nothing scrolls sideways at 900px: the pane's sideways scroll is not probed");
    let rail_900 = rail_rect(&mut app, "appearance").map_or(0.0, |r| r.2);
    ctx.ok(
        rail_900 >= 200.0,
        format!("at 900px the rail has its names ({rail_900}px)"),
    );

    // Under 760px of the frame: the rail is icons, and the page names are hidden.
    resize(&mut app, 700.0, 800.0);
    let narrow_rail = ctx.until("under 760px the rail is icons", 3_000, &mut || {
        rail_rect(&mut app, "appearance").is_some_and(|r| r.2 <= 60.0)
    });
    let rail_700 = rail_rect(&mut app, "appearance").map_or(0.0, |r| r.2);
    ctx.ok(
        narrow_rail,
        format!("under 760px the rail is icons ({rail_700}px)"),
    );
    ctx.ok(
        rows_now(&mut app)["narrow"] == json!(true),
        "with the page names hidden",
    );
    ctx.skip("nothing scrolls sideways under 760px: the pane's sideways scroll is not probed");
    ctx.skip("the magnifier opens the field over the pane: the field's width is not in the bridge");

    resize(&mut app, 1600.0, 1000.0);
    app.close();
    Ok(())
}

/// PT: run.mjs:5510-5606. Find a setting: the index holds every row, the field owns Escape only
/// while it holds text, the result list is walked with the keyboard, a chosen result opens its page
/// and flashes its row, Escape clears, and a word that matches nothing says so.
pub fn settings_search(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = launch_with_tab(ctx)?;
    ctx.until("one tab open", 15_000, &mut || tabs(&mut app) == 1);
    resize(&mut app, 1600.0, 1000.0);
    // The index: every row of the catalogue that is findable on this PC (the NVIDIA-only row and
    // the Dictation page, which is not wired until M8, are not indexed).
    let cat = probe(&mut app, "settings.rows", json!({ "catalogue": true }));
    let indexed: Vec<(String, String)> = cat["rows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|r| {
            !r["only_nvidia"].as_bool().unwrap_or(false) && str_of(r, "page") != "dictation"
        })
        .map(|r| (str_of(r, "id").to_string(), str_of(r, "label").to_string()))
        .collect();
    ctx.ok(
        indexed.len() >= 30,
        format!(
            "the index covers every row drawn on this PC ({})",
            indexed.len()
        ),
    );
    if !open_settings(ctx, &mut app) {
        ctx.ok(false, "the cog opens Settings");
        app.close();
        return Ok(());
    }
    let s0 = search_now(&mut app);
    ctx.ok(
        str_of(&s0, "status").is_empty() && s0["searching"] == json!(false),
        "with nothing typed there is no status line",
    );
    ctx.ok(
        s0["owns_escape"] == json!(false),
        "an empty field does not claim Escape",
    );
    focus_find(&mut app);
    let _ = app.session.type_text("font");
    let typed = ctx.until("the field holds the text", 3_000, &mut || {
        search_now(&mut app)["owns_escape"] == json!(true)
    });
    // The field is focused by its click (settings.find_rect) and takes the typed letters: a key over
    // the Settings page reaches Slint (appctl `on_key`, the NotAChord arm; fixed in 75c510e, M10 D17).
    ctx.ok(typed, "a field holding text does");
    let _ = app.session.key("Escape");
    ctx.until("the field is clear", 3_000, &mut || {
        search_now(&mut app)["owns_escape"] == json!(false)
    });
    // DROPPED: "a control names itself by its row's label" (an accessible group name) because
    // Slint's accessible names are not probed; Task 45 asserts them.

    // EVERY ROW, BY ITS OWN LABEL: type the label, the first result is the row, choose it, and the
    // row is on screen and flashed on the page it lives on.
    let mut misses: Vec<String> = Vec::new();
    for (id, label) in &indexed {
        focus_find(&mut app);
        let _ = app.session.type_text(label);
        let found = ctx.until(&format!("{id} is found"), 3_000, &mut || {
            search_now(&mut app)["hits"]
                .as_array()
                .is_some_and(|h| !h.is_empty())
        });
        if !found {
            misses.push(format!("{id}: nothing found"));
            let _ = app.session.key("Escape");
            continue;
        }
        let first = search_now(&mut app)["hits"][0]["id"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if first != *id {
            misses.push(format!("{id}: first result is {first}"));
            let _ = app.session.key("Escape");
            continue;
        }
        let _ = app.session.key("ArrowDown");
        let _ = app.session.key("Enter");
        let landed = ctx.until(&format!("{id} is flashed on screen"), 4_000, &mut || {
            row_landed(&mut app, id)
        });
        if !landed {
            misses.push(format!("{id}: not on screen and flashed"));
        }
        ctx.ok(
            str_of(&search_now(&mut app), "query").is_empty(),
            format!("{id}: choosing a result clears the field"),
        );
    }
    ctx.ok(
        misses.is_empty(),
        format!("every row is found by its label and opened ({misses:?})"),
    );

    // KEYBOARD ONLY: the field, Down, Enter. The focus itself is not probed (Slint exposes no focus
    // query), so the row being on screen and flashed on the page it lives on stands for it.
    focus_find(&mut app);
    let _ = app.session.type_text("explorer");
    let counted = ctx.until("a status line says how many", 3_000, &mut || {
        str_of(&search_now(&mut app), "status").contains("result")
    });
    let status = str_of(&search_now(&mut app), "status").to_string();
    ctx.ok(counted, format!("a status line says how many ({status})"));
    let _ = app.session.key("ArrowDown");
    let at_first = ctx.until("Down moves to the first result", 2_000, &mut || {
        search_now(&mut app)["hit_cursor"] == json!(0)
    });
    ctx.ok(at_first, "Down moves to the first result");
    let _ = app.session.key("Enter");
    let opened = ctx.until("Enter opens the row", 4_000, &mut || {
        row_landed(&mut app, "explorer-verb")
    });
    ctx.ok(
        opened,
        "Enter opens it with the keyboard on the row's control",
    );
    let chosen = search_now(&mut app)["rail_chosen"].clone();
    let on_page = page_now(&mut app).as_deref() == Some("terminal");
    ctx.ok(
        on_page && chosen == json!(["terminal"]),
        format!("on the page that holds it ({chosen})"),
    );

    // While results are up no rail page is chosen. Escape clears the field and the status line.
    focus_find(&mut app);
    let _ = app.session.type_text("colour");
    ctx.until("results for colour", 3_000, &mut || {
        !str_of(&search_now(&mut app), "status").is_empty()
    });
    let during = search_now(&mut app);
    ctx.ok(
        during["searching"] == json!(true) && during["rail_chosen"] == json!([]),
        "while results are up no page is chosen in the rail",
    );
    let _ = app.session.key("Escape");
    let cleared = ctx.until("Escape clears the field and the status", 3_000, &mut || {
        let s = search_now(&mut app);
        str_of(&s, "query").is_empty() && str_of(&s, "status").is_empty()
    });
    ctx.ok(cleared, "Escape clears the field and the status line goes");

    // Nothing found.
    focus_find(&mut app);
    let _ = app.session.type_text("zebra");
    let none = ctx.until(
        "a word that matches nothing says No results",
        3_000,
        &mut || str_of(&search_now(&mut app), "status") == "No results",
    );
    ctx.ok(none, "a word that matches nothing says No results");
    let nothing = str_of(&search_now(&mut app), "nothing").to_string();
    ctx.ok(
        nothing.contains("Nothing matches zebra"),
        format!("and the pane says what was not found ({nothing})"),
    );
    let _ = app.session.key("Escape");

    resize(&mut app, 1600.0, 1000.0);
    app.close();
    Ok(())
}

/// True when row `id` is drawn on the page in front, is inside the visible pane and is flashing.
fn row_landed(app: &mut App, id: &str) -> bool {
    let v = rows_now(app);
    let pane_h = v["pane_h"].as_f64().unwrap_or(0.0);
    let scroll = v["scroll_y"].as_f64().unwrap_or(0.0);
    let sections = v["sections"].as_array().cloned().unwrap_or_default();
    v["rows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .any(|r| {
            if str_of(r, "id") != id {
                return false;
            }
            let panel_y = sections
                .iter()
                .find(|s| str_of(s, "id") == str_of(r, "section"))
                .and_then(|s| s["panel_y"].as_f64())
                .unwrap_or(0.0);
            let top = panel_y + r["y"].as_f64().unwrap_or(0.0) - scroll;
            let h = r["h"].as_f64().unwrap_or(0.0);
            top + h > 0.0 && top < pane_h && r["flash"].as_f64().unwrap_or(0.0) > 0.0
        })
}

/// PT: run.mjs:5607-5640. The rail by keyboard: Enter opens the page the arrows reach, and the rail
/// says which page is chosen, one at a time. The focus moves (Tab, Up, Down, Home, End) are not
/// probed (Slint exposes no focus query), and the landmark is a DOM role (see the DROPPED line).
pub fn settings_keys(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = launch_with_tab(ctx)?;
    ctx.until("one tab open", 15_000, &mut || tabs(&mut app) == 1);
    if !open_settings(ctx, &mut app) {
        ctx.ok(false, "the cog opens Settings");
        app.close();
        return Ok(());
    }
    focus_find(&mut app);
    ctx.skip("Tab from the field lands on the rail's first page: focus is not probed");
    let _ = app.session.key("Tab");
    ctx.skip("Down walks the rail: focus is not probed");
    ctx.skip("End jumps to the last page: focus is not probed");
    ctx.skip("Home jumps to the first page: focus is not probed");
    let _ = app.session.key("ArrowDown");
    let _ = app.session.key("ArrowDown");
    let _ = app.session.key("Enter");
    let agents = ctx.until("Enter opens the agents page", 5_000, &mut || {
        page_now(&mut app).as_deref() == Some("agents")
    });
    let chosen = search_now(&mut app)["rail_chosen"].clone();
    ctx.ok(
        agents && chosen == json!(["agents"]),
        format!("Enter opens it, and the rail says it is the page ({chosen})"),
    );
    let one = search_now(&mut app)["rail_chosen"]
        .as_array()
        .map_or(0, Vec::len);
    ctx.ok(one == 1, "one page at a time");
    // DROPPED: the rail is a navigation landmark named "Settings pages" (a DOM role); Slint has no
    // landmark, and Task 45 asserts the accessible names.
    ctx.skip("Tab goes on from the rail into the page: focus is not probed");

    app.close();
    Ok(())
}
