//! Chrome scenarios (Task 21): theme, newTabFolder, systemMenu, cogToggle, titleBar, settingsPage.
//! Each follows its Prism Terminal line (PT: run.mjs); a body says where Flare differs.

use flare_e2e_proto::{Reply, Request};
use serde_json::{json, Value};

use super::driver;
use crate::ctx::{App, Ctx, Launch};

/// The number of tabs the app shows now (0 when the probe fails).
fn tabs(app: &mut App) -> usize {
    app.session.state().map(|s| s.tabs.len()).unwrap_or(0)
}

/// A probe's answer, `Null` when it fails (a check on it then reads false).
fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// A `ui.rect` answer in css pixels as (x, y, w, h), or `None` when the app says there is none.
fn rect(app: &mut App, name: &str) -> Option<(f64, f64, f64, f64)> {
    boxed(&probe(app, "ui.rect", json!({ "name": name })))
}

/// A rectangle value (`x`, `y`, `w`, `h`) as a tuple.
fn boxed(v: &Value) -> Option<(f64, f64, f64, f64)> {
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

/// The Settings probe's answer: `{ open, front, page, rail }`.
fn settings(app: &mut App) -> Value {
    probe(app, "settings.state", Value::Null)
}

/// The page the Settings tab holds while it is in front (`None` otherwise).
fn settings_page_of(app: &mut App) -> Option<String> {
    settings(app)["page"].as_str().map(str::to_string)
}

/// The scale and the client width in css pixels, and the window's device origin.
fn window_css(app: &mut App) -> (f64, f64, (i64, i64)) {
    let st = probe(app, "app.state", Value::Null);
    let scale = app
        .session
        .state()
        .map(|s| f64::from(s.scale))
        .unwrap_or(1.0);
    let w = st["window"]["w"].as_f64().unwrap_or(0.0);
    let origin = (
        st["window"]["x"].as_i64().unwrap_or(0),
        st["window"]["y"].as_i64().unwrap_or(0),
    );
    (scale, w / scale, origin)
}

/// Opens the first tab from the start screen, clicking the New row where the layout put it.
fn first_tab(ctx: &mut Ctx, app: &mut App) -> bool {
    let start = probe(app, "start.state", Value::Null);
    let (x, y) = (
        start["new_centre"]["x"].as_f64().unwrap_or(0.0) as f32,
        start["new_centre"]["y"].as_f64().unwrap_or(0.0) as f32,
    );
    if app.session.click(x, y).is_err() {
        return false;
    }
    ctx.until("the first tab", 15_000, &mut || tabs(app) == 1)
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
        Ok(Reply::Pixel { rgba }) => Some([rgba[0], rgba[1], rgba[2]]),
        _ => None,
    }
}

/// The ground the theme gives the window, as the rgb the probe reports.
fn ground_rgb(app: &mut App) -> [u8; 3] {
    let g = probe(app, "window.ground", Value::Null);
    rgb_of_hex(g["ground"].as_str().unwrap_or("#000000"))
}

/// A css point inside the tab strip's empty space, at its right end.
fn strip_empty_css(app: &mut App) -> Option<(f64, f64)> {
    let s = rect(app, "strip")?;
    Some((s.0 + s.2 - 8.0, s.1 + s.3 / 2.0))
}

/// The scale of the app (device pixels per css pixel).
fn scale_of(app: &mut App) -> f64 {
    app.session
        .state()
        .map(|s| f64::from(s.scale))
        .unwrap_or(1.0)
}

/// How many system menu requests the app has counted (Task 18's recording seam).
fn sysmenu_count(app: &mut App) -> u64 {
    probe(app, "window.sysmenu_requests", Value::Null)["count"]
        .as_u64()
        .unwrap_or(u64::MAX)
}

/// PT: run.mjs:3033-3048. The window refuses the system menu: a right click on the strip's empty
/// space calls none, and Alt+Space asks for exactly one.
pub fn system_menu(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        ..Launch::default()
    })?;
    if !first_tab(ctx, &mut app) {
        ctx.ok(false, "the system menu check needs a tab, and none opened");
        app.close();
        return Ok(());
    }
    let before = sysmenu_count(&mut app);
    if let Some((x, y)) = strip_empty_css(&mut app) {
        app.session.right_click(x as f32, y as f32)?;
    }
    let after_right_click = sysmenu_count(&mut app);
    app.session.key("Alt+Space")?;
    let asked = ctx.until("the system menu request", 5_000, &mut || {
        sysmenu_count(&mut app) > after_right_click
    });
    ctx.settle(50);
    let after_alt = sysmenu_count(&mut app);
    ctx.ok(
        before == 0 && after_right_click == 0 && asked && after_alt == 1,
        format!(
            "the window refuses the system menu on a right click ({before} -> {after_right_click}), and Alt+Space asks for exactly one ({after_alt})"
        ),
    );
    app.close();
    Ok(())
}

/// PT: run.mjs:3121-3140. The cog toggles Settings as a tab of its own; Ctrl+, does the same.
pub fn cog_toggle(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        ..Launch::default()
    })?;
    let one = first_tab(ctx, &mut app);
    ctx.ok(one, "one tab");
    click_rect(&mut app, "cog");
    let up = ctx.until("the cog opens Settings", 4_000, &mut || {
        settings(&mut app)["open"] == true
    });
    ctx.ok(up, "the cog opens Settings");
    ctx.ok(tabs(&mut app) == 2, "as a tab of its own");
    click_rect(&mut app, "cog");
    let down = ctx.until("the cog again closes it", 4_000, &mut || {
        settings(&mut app)["open"] == false
    });
    ctx.ok(down, "the cog again closes it");
    ctx.ok(
        tabs(&mut app) == 1,
        "and its tab with it (one tab left, the shell)",
    );
    let front_back = ctx.until("the shell is in front again", 3_000, &mut || {
        settings(&mut app)["front"] == false
    });
    ctx.ok(front_back, "the shell is in front again");
    app.session.key("Ctrl+Comma")?;
    let by_key = ctx.until("Ctrl+comma opens it", 4_000, &mut || {
        settings(&mut app)["open"] == true
    });
    ctx.ok(by_key, "Ctrl+comma opens it");
    app.session.key("Ctrl+Comma")?;
    let by_key_off = ctx.until("and closes it", 4_000, &mut || {
        settings(&mut app)["open"] == false
    });
    ctx.ok(by_key_off, "and closes it");
    app.close();
    Ok(())
}

/// PT: run.mjs:3199-3244. The tabs sit in the top row with the buttons at its right end, one row
/// above the terminal, and a drag from the empty strip moves the window.
pub fn title_bar(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        ..Launch::default()
    })?;
    // The first tab is the precondition; the check below is whether the second one opens.
    let _first = first_tab(ctx, &mut app);
    app.session.key("Ctrl+T")?;
    let two = ctx.until("two tabs open", 15_000, &mut || tabs(&mut app) == 2);
    ctx.ok(two, "two tabs open");

    // PT: the tabs are in the top row (the strip starts at the window's top, inside the bar).
    let strip = rect(&mut app, "strip");
    let top_row = strip.is_some_and(|s| s.1.abs() < 0.5 && s.3 > 0.0);
    ctx.ok(top_row, format!("the tabs are in the top row ({strip:?})"));

    // PT: the settings, minimise and close buttons sit at the right end of that row.
    let (_, css_w, _) = window_css(&mut app);
    let close = rect(&mut app, "close");
    let cog = rect(&mut app, "cog");
    let min = rect(&mut app, "min");
    #[cfg(windows)]
    let at_end = match (close, cog, min) {
        (Some(c), Some(_), Some(_)) => css_w - (c.0 + c.2) <= 12.0,
        _ => false,
    };
    // macOS: the window buttons are AppKit's traffic lights on the left (outside what the app draws,
    // `ui.rect` refuses them), and the cog is what ends the row (rule 339).
    #[cfg(not(windows))]
    let at_end =
        close.is_none() && min.is_none() && cog.is_some_and(|c| css_w - (c.0 + c.2) <= 12.0);
    ctx.ok(
        at_end,
        format!("and the settings, minimise and close buttons sit at its right end ({close:?})"),
    );

    // PT: one row above the terminal (the grid starts under the bar: 20 < y <= 40 css px).
    let grid = rect(&mut app, "grid");
    let one_row = grid.is_some_and(|g| g.1 > 20.0 && g.1 <= 40.0);
    ctx.ok(one_row, format!("one row above the terminal ({grid:?})"));

    // PT: the strip's empty space moves the window (a drag that starts there moves the origin).
    // DIFF-14 (PROPOSED): Flare's bar moves the window through Slint's WindowMoveArea, which asks
    // winit to start Windows' move loop, and a synthetic Drag cannot start it (the window sits at
    // (-32000, -32000) and its origin never changes). The check cannot be made, so it is skipped
    // with that reason, as verticalTabs skips its own.
    ctx.skip("the strip's empty space moves the window: a synthetic Drag cannot start Windows' move loop (DIFF-14)");
    // PT: there is no Show title bar setting row: the catalogue of every Settings row has none.
    let no_row = !driver::catalogue(&mut app).iter().any(|r| {
        r["label"]
            .as_str()
            .is_some_and(|l| l.to_lowercase().contains("title bar"))
            || r["id"].as_str().is_some_and(|i| i.contains("title-bar"))
    });
    ctx.ok(no_row, "no Show title bar row in Settings");
    // DROPPED: an old stored "shown" value for a title bar setting has nothing to change here,
    // because Flare never had that key (M7's import map drops it).

    // PT: every tab closed; the top row still holds the buttons with no tabs.
    for _ in 0..6 {
        if tabs(&mut app) == 0 {
            break;
        }
        app.session.key("Ctrl+W")?;
        ctx.settle(50);
    }
    let closed = ctx.until("every tab closed", 5_000, &mut || tabs(&mut app) == 0);
    ctx.ok(closed, "every tab closed");
    let empty_cog = rect(&mut app, "cog");
    let empty_close = rect(&mut app, "close");
    #[cfg(windows)]
    let held = match (empty_cog, empty_close) {
        (Some(c), Some(x)) => c.1 + c.3 <= 36.0 && x.1 + x.3 <= 36.0,
        _ => false,
    };
    #[cfg(not(windows))]
    let held = empty_close.is_none() && empty_cog.is_some_and(|c| c.1 + c.3 <= 36.0);
    ctx.ok(held, "with no tabs the top row still holds the buttons");
    app.close();
    Ok(())
}

/// PT: run.mjs:1146-1175. A chosen folder replaces the user's own, and can be given back.
pub fn new_tab_folder(ctx: &mut Ctx) -> Result<(), String> {
    let chosen = ctx.ensure_world().root.join(CHOSEN);
    std::fs::create_dir_all(&chosen).map_err(|e| e.to_string())?;
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        env: vec![(
            "FLARE_E2E_PICK".to_string(),
            chosen.to_string_lossy().into_owned(),
        )],
        ..Launch::default()
    })?;
    // PT: the title bar has a settings cog and no menu.
    let cog = rect(&mut app, "cog").is_some();
    ctx.ok(cog, "the title bar has a settings cog");
    // PT: opening in a folder is the default. With no folder stored, the first tab opens in the
    // user's own folder, so its tooltip is that folder's full path.
    let home = crate::world::user_home(ctx);
    let _opened = first_tab(ctx, &mut app);
    let tip = probe(&mut app, "app.state", Value::Null)["tabs"][0]["tooltip"]
        .as_str()
        .unwrap_or("")
        .to_string();
    ctx.ok(
        !home.is_empty() && tip.eq_ignore_ascii_case(&home),
        format!("opening in a folder is the default ({tip:?})"),
    );
    // PT: Choose folder remembers the folder; the + opens a tab there at once; in the chosen
    // folder; and "Use my user folder" gives the default back. They run on the Terminal page's
    // folder row; the folder picker answers from the FLARE_E2E_PICK seam, with no dialog.
    let settings_file = ctx.ensure_world().profile.join("settings.json");
    let there = driver::goto_row(ctx, &mut app, "newtab-mode");
    let before = shell_tabs(&mut app);
    // The last button of the row is Choose folder: a point 30 px in from the row's right end.
    let clicked = click_row_at(&mut app, "newtab-mode", 30.0);
    let remembered = ctx.poll(6_000, &mut || {
        std::fs::read_to_string(&settings_file)
            .is_ok_and(|s| s.contains("flare.newtab.folder") && s.contains(CHOSEN))
    });
    ctx.ok(
        there && clicked && remembered,
        "Choose folder remembers the folder",
    );
    // The + and Ctrl+T are one action (appctl): a new tab opens at once, with nothing asked.
    app.session.key("Ctrl+T")?;
    let at_once = ctx.until("a new tab at once", 15_000, &mut || {
        shell_tabs(&mut app).len() == before.len() + 1
    });
    ctx.ok(at_once, "the + opens a tab at once");
    let tip = shell_tabs(&mut app)
        .last()
        .and_then(|t| t["tooltip"].as_str().map(str::to_string))
        .unwrap_or_default();
    ctx.ok(
        tip.to_lowercase().contains(CHOSEN),
        format!("in the chosen folder ({tip:?})"),
    );
    // "Use my user folder" is the button before Choose folder: a point well left of it.
    let back = driver::goto_row(ctx, &mut app, "newtab-mode")
        && probe(&mut app, "diag.buttons", json!({ "id": "newtab-mode" }))["buttons"]
            .as_array()
            .is_some_and(|b| b.len() == 2)
        && click_row_at(&mut app, "newtab-mode", 200.0);
    let given_back = ctx.poll(6_000, &mut || {
        std::fs::read_to_string(&settings_file)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .is_some_and(|v| v["flare.newtab.folder"].as_str().is_none_or(str::is_empty))
    });
    let count = shell_tabs(&mut app).len();
    app.session.key("Ctrl+T")?;
    let opened = ctx.until("a tab in the user's folder", 15_000, &mut || {
        shell_tabs(&mut app).len() == count + 1
    });
    let tip = shell_tabs(&mut app)
        .last()
        .and_then(|t| t["tooltip"].as_str().map(str::to_string))
        .unwrap_or_default();
    ctx.ok(
        back && given_back && opened && !home.is_empty() && tip.eq_ignore_ascii_case(&home),
        format!("Use my user folder gives the default back ({tip:?})"),
    );
    app.close();
    Ok(())
}

/// The folder the picker seam answers with (also the end of the new tab's tooltip), in lower case.
const CHOSEN: &str = "chosen-folder";

/// The tabs that are shells or agents (Settings left out), as `app.state` lists them.
fn shell_tabs(app: &mut App) -> Vec<Value> {
    probe(app, "app.state", Value::Null)["tabs"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter(|x| x["kind"] != json!("settings"))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Clicks the Settings row `id`, `from_right` css px in from its right end, at its middle.
fn click_row_at(app: &mut App, id: &str, from_right: f64) -> bool {
    let r = driver::row(app, id);
    let Some((x, y, w, h)) = boxed(&r["rect"]) else {
        return false;
    };
    app.session
        .click((x + w - from_right) as f32, (y + h / 2.0) as f32)
        .is_ok()
}

/// The rail item `id` in css pixels, from the `settings.rail` probe.
fn rail_rect(app: &mut App, id: &str) -> Option<(f64, f64, f64, f64)> {
    boxed(&probe(app, "settings.rail", json!({ "id": id })))
}

/// PT: run.mjs:3249-3269. Settings keeps its page while its tab stays open; closing the tab and
/// opening it again starts on Appearance.
pub fn settings_page(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        ..Launch::default()
    })?;
    let one = first_tab(ctx, &mut app);
    ctx.ok(one, "one tab open");
    click_rect(&mut app, "cog");
    let appearance = ctx.until("Settings opens on Appearance", 5_000, &mut || {
        settings_page_of(&mut app).as_deref() == Some("appearance")
    });
    ctx.ok(appearance, "Settings opens on Appearance");
    if let Some(r) = rail_rect(&mut app, "dictation") {
        let (x, y) = centre(r);
        app.session.click(x, y)?;
    }
    let dictation = ctx.until("Dictation picked", 5_000, &mut || {
        settings_page_of(&mut app).as_deref() == Some("dictation")
    });
    ctx.ok(dictation, "Dictation picked");
    // The shell tab (tab 0) comes to the front: Settings unmounts and keeps its page.
    if let Some(r) = rect(&mut app, "tab:0") {
        let (x, y) = centre(r);
        app.session.click(x, y)?;
    }
    let unmounted = ctx.until(
        "the shell is in front, Settings unmounted",
        5_000,
        &mut || settings(&mut app)["front"] == false && settings(&mut app)["page"].is_null(),
    );
    ctx.ok(unmounted, "the shell is in front, Settings unmounted");
    if let Some(r) = rect(&mut app, "tab:1") {
        let (x, y) = centre(r);
        app.session.click(x, y)?;
    }
    let kept = ctx.until("back on Settings it is still Dictation", 5_000, &mut || {
        settings_page_of(&mut app).as_deref() == Some("dictation")
    });
    let now = settings_page_of(&mut app);
    ctx.ok(
        kept,
        format!("back on Settings it is still Dictation ({now:?})"),
    );
    // Close the Settings tab (it is in front), then open it again.
    app.session.key("Ctrl+W")?;
    let closed = ctx.until("the Settings tab closed", 5_000, &mut || {
        settings(&mut app)["open"] == false
    });
    ctx.ok(closed, "the Settings tab closed");
    click_rect(&mut app, "cog");
    let again = ctx.until("opened again it starts on Appearance", 5_000, &mut || {
        settings_page_of(&mut app).as_deref() == Some("appearance")
    });
    let page = settings_page_of(&mut app);
    ctx.ok(
        again,
        format!("opened again it starts on Appearance ({page:?})"),
    );
    app.close();
    Ok(())
}

/// PT: run.mjs:1082-1143. Dark is the default and the chrome paints the theme's ground; a light
/// preset turns it light, and the strip follows.
pub fn theme(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        ..Launch::default()
    })?;
    // The first tab is the precondition the port waits for; the checks below read it.
    let _one = first_tab(ctx, &mut app);
    // macOS: pwsh starts later than the first frame, and until it prints the block cursor sits on the
    // cell the frame check reads; wait for its prompt (Windows' ConPTY keeps the cursor hidden there).
    #[cfg(target_os = "macos")]
    {
        let front = app
            .session
            .state()
            .ok()
            .and_then(|s| s.front)
            .unwrap_or_default();
        let _ = crate::world::wait_for_prompt(
            &mut app,
            &front,
            crate::world::Shell::PowerShell,
            crate::world::prompt_timeout(crate::world::Shell::PowerShell),
        );
    }
    let dark = probe(&mut app, "window.ground", Value::Null)["light"] == false;
    ctx.ok(dark, "the default theme is dark");
    let ground = ground_rgb(&mut app);
    let _ = ctx.shot(&mut app, "theme-dark");
    let k = scale_of(&mut app);
    let strip = strip_empty_css(&mut app).and_then(|(x, y)| pixel(&mut app, x * k, y * k));
    ctx.ok(
        strip == Some(ground),
        format!("the strip's empty space paints the theme's ground ({strip:?} vs {ground:?})"),
    );
    let frame = rect(&mut app, "grid").and_then(|g| pixel(&mut app, g.0 * k + 6.0, g.1 * k + 6.0));
    ctx.ok(
        frame == Some(ground),
        format!("the frame round the rows and the rows paint the ground ({frame:?})"),
    );
    // DROPPED: "a strip under the last row" is xterm sizing its rows in a web view; Flare's grid
    // is drawn at its own size, so there is no strip under the last row to get wrong.
    app.close();

    // The light half: a second launch with the light preset (picking a card is not probed yet).
    let mut light = ctx.launch(Launch {
        renderer: Some("software"),
        args: vec!["--theme".into(), "paper".into()],
        ..Launch::default()
    })?;
    let _one = first_tab(ctx, &mut light);
    let g = probe(&mut light, "window.ground", Value::Null);
    ctx.ok(g["light"] == true, "a light preset turns the chrome light");
    let lum = g["luminance"].as_f64().unwrap_or(0.0);
    ctx.ok(
        lum > 0.4,
        format!("the ground really is light (luminance {lum:.2})"),
    );
    let light_ground = rgb_of_hex(g["ground"].as_str().unwrap_or("#000000"));
    let kl = scale_of(&mut light);
    // The pixel reads answer from the last snapshot, so the light window takes its own.
    let _ = ctx.shot(&mut light, "theme-light");
    let follows = strip_empty_css(&mut light).and_then(|(x, y)| pixel(&mut light, x * kl, y * kl));
    ctx.ok(
        follows == Some(light_ground) && light_ground != ground,
        format!("the strip follows the theme ({follows:?})"),
    );
    // PT: main was told the window's ground (a light ground starts with #f).
    let told = g["ground"].as_str().unwrap_or("").to_lowercase();
    ctx.ok(
        told.starts_with("#f"),
        format!("main was told the window's ground ({told})"),
    );
    light.close();
    Ok(())
}
