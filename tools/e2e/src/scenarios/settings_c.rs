//! Settings C scenarios (Task 35): colourPicker, pickedGround, opacityAlpha, seeThrough. Each follows
//! its Prism Terminal lines (PT: run.mjs 6014, 6289, 6350, 6460); a body says where Flare differs.
//!
//! Flare answers through the `picker.*` probes (e2e_probes/settings_c.rs: the popover, a row's
//! control, the wall, the editor, the layers over the page), `glass.paint` (the Slint theme's
//! grounds with their alpha), `agent.working` (the working mark's colour), the stored settings
//! (`<profile>\settings.json`, which PT reads as localStorage) and snapshots for the pixel checks.
//! What Slint does not export is a `ctx.skip` with its reason, never a pass.
//!
//! Two facts shape the glass scenarios. The software renderer paints a solid window (DIFF-5) and
//! disables the See-through slider (`GlassSupport::NeedsGraphicsRenderer`), so what needs the slider
//! is `skip`ped on software with that reason and runs on the GPU renderer. And the Settings tab has
//! no shell in front, so a typed letter is consumed before Slint sees it (appctl `on_key`): the
//! checks that type into a code field report a FLARE-DEFECT line until that is fixed.

use std::collections::BTreeMap;
use std::path::Path;

use flare_e2e_proto::{PointerAction, Request};
use serde_json::{json, Value};

use crate::ctx::{App, Ctx, Launch};
use crate::pixels::{contrast, Rgb};
use crate::world::{prompt_timeout, wait_for_prompt, Shell};

/// A css rectangle: x, y, width, height.
type R = (f64, f64, f64, f64);

/// 225 percent (96 is 100 percent): the scale the pixel checks run at.
const DPI_225: u32 = 216;
/// The reason every slider-dependent check is skipped on the software renderer.
const SOFTWARE_SKIP: &str = "the software renderer paints a solid window (DIFF-5) and disables the See-through slider; this runs on the GPU renderer";

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

fn click_at(app: &mut App, p: (f32, f32)) -> bool {
    app.session.click(p.0, p.1).is_ok()
}

fn click_r(app: &mut App, v: &Value) -> bool {
    match rect_of(v) {
        Some(r) => click_at(app, centre(r)),
        None => false,
    }
}

fn press(app: &mut App, chord: &str) -> bool {
    app.session.key(chord).is_ok()
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn tabs_of(app: &mut App) -> Vec<Value> {
    probe(app, "app.state", Value::Null)["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn scale_of(app: &mut App) -> f64 {
    app.session
        .state()
        .ok()
        .map(|s| f64::from(s.scale))
        .filter(|k| *k > 0.0)
        .unwrap_or(1.0)
}

fn resize(app: &mut App, w: f32, h: f32) -> bool {
    app.session.call(&Request::Resize { w, h }).is_ok()
}

/// True when the run is on the GPU renderer (the glass checks need it).
fn on_gpu(app: &mut App) -> bool {
    app.hello()
        .map(|h| h.renderer.to_lowercase().starts_with("gpu"))
        .unwrap_or(false)
}

/// The stored settings (`<profile>\settings.json`, a JSON object of strings), or empty.
fn prefs_in(profile: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(profile.join("settings.json")).unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    v.as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn pref(app: &App, key: &str) -> Option<String> {
    prefs_in(&app.profile).remove(key)
}

/// Writes (merges) settings into a closed profile, so a launch starts from them.
fn seed_prefs(profile: &Path, pairs: &[(&str, &str)]) -> Result<(), String> {
    let mut map = prefs_in(profile);
    for (k, v) in pairs {
        map.insert((*k).to_string(), (*v).to_string());
    }
    let text = serde_json::to_string(&map).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(profile).map_err(|e| e.to_string())?;
    std::fs::write(profile.join("settings.json"), text).map_err(|e| e.to_string())
}

/// True for `#rrggbbaa`.
fn is_hex8(s: &str) -> bool {
    s.len() == 9 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// A code as drawn or stored, without its `#` and in lower case.
fn norm_code(s: &str) -> String {
    s.trim_start_matches('#').to_lowercase()
}

/// Holds the scenario still for `ms`, in 50 ms "no change" steps (the most `settle` allows).
fn hold(ctx: &mut Ctx, ms: u64) {
    for _ in 0..(ms / 50) {
        ctx.settle(50);
    }
}

/// A stored value after the store's write delay has passed: for "nothing was stored" checks.
fn settled_pref(ctx: &mut Ctx, app: &App, key: &str) -> Option<String> {
    hold(ctx, 450);
    pref(app, key)
}

/// Waits for a stored value to satisfy `want`; true when it did.
fn pref_becomes(
    ctx: &mut Ctx,
    app: &App,
    key: &str,
    what: &str,
    want: &dyn Fn(Option<&str>) -> bool,
) -> bool {
    ctx.until(what, 4_000, &mut || want(pref(app, key).as_deref()))
}

// ---- the page and the popover --------------------------------------------------------------------------

fn settings_front(app: &mut App) -> bool {
    probe(app, "settings.state", Value::Null)["front"] == json!(true)
}

fn open_settings(ctx: &mut Ctx, app: &mut App) -> bool {
    let cog = probe(app, "ui.rect", json!({ "name": "cog" }));
    if !click_r(app, &cog) {
        return false;
    }
    ctx.until("Settings is in front", 8_000, &mut || settings_front(app))
}

fn rail_rect(app: &mut App, id: &str) -> Option<R> {
    rect_of(&probe(app, "settings.rail", json!({ "id": id })))
}

fn page_now(app: &mut App) -> Option<String> {
    probe(app, "settings.rows", json!({}))["page"]
        .as_str()
        .map(str::to_string)
}

fn goto_page(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    if let Some(r) = rail_rect(app, id) {
        click_at(app, centre(r));
    }
    ctx.until(&format!("the {id} page is drawn"), 6_000, &mut || {
        page_now(app).as_deref() == Some(id)
            && probe(app, "settings.rows", json!({}))["count"]
                .as_u64()
                .unwrap_or(0)
                > 0
    })
}

fn row(app: &mut App, id: &str) -> Value {
    probe(app, "picker.row", json!({ "id": id }))
}

fn state(app: &mut App) -> Value {
    probe(app, "picker.state", Value::Null)
}

fn picker_open(app: &mut App) -> bool {
    state(app)["open"] == json!(true)
}

fn wait_picker(ctx: &mut Ctx, app: &mut App, open: bool, what: &str) -> bool {
    ctx.until(what, 4_000, &mut || picker_open(app) == open)
}

/// Wheels the pane until `find` says its box is inside the pane. `find` answers the box's top
/// edge and whether it is visible, or None while the thing is not drawn.
fn bring_into_view(
    ctx: &mut Ctx,
    app: &mut App,
    what: &str,
    find: &mut dyn FnMut(&mut App) -> Option<(f64, bool)>,
) -> bool {
    let grid = rect_of(&probe(app, "ui.rect", json!({ "name": "grid" })))
        .unwrap_or((0.0, 36.0, 1000.0, 600.0));
    let at = ((grid.0 + 500.0) as f32, (grid.1 + grid.3 / 2.0) as f32);
    let mid = grid.1 + grid.3 / 2.0;
    ctx.until(
        &format!("{what} to be in view"),
        12_000,
        &mut || match find(app) {
            Some((_, true)) => true,
            Some((y, false)) => {
                let dy = if y > mid { -160.0 } else { 160.0 };
                let _ = app.session.call(&Request::Pointer {
                    x: at.0,
                    y: at.1,
                    action: PointerAction::Wheel { dx: 0.0, dy },
                });
                false
            }
            None => false,
        },
    )
}

/// Scrolls a row into view; true when it is.
fn reveal(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    bring_into_view(ctx, app, &format!("row {id}"), &mut |a| {
        let r = row(a, id);
        let y = r["rect"]["y"].as_f64()?;
        Some((y, r["visible"] == json!(true)))
    })
}

/// Opens the colour popover of a row by its swatch.
fn open_swatch(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    if !reveal(ctx, app, id) {
        return false;
    }
    let r = row(app, id);
    if !click_r(app, &r["control"]) {
        return false;
    }
    wait_picker(ctx, app, true, &format!("the {id} picker to open"))
}

/// Presses Escape and waits for the popover to go.
fn escape_picker(ctx: &mut Ctx, app: &mut App) -> bool {
    press(app, "Escape");
    wait_picker(ctx, app, false, "the picker to close")
}

fn alpha_now(app: &mut App) -> i64 {
    state(app)["alpha_now"].as_i64().unwrap_or(-1)
}

/// Gives the alpha bar the keys (the popover opens with the square focused; Tab walks the square,
/// the hue bar, the alpha bar), then walks it down with Shift+Left (ten) and Left (one) to `to`.
/// Returns where it stopped. The popover must have just opened.
fn walk_alpha(ctx: &mut Ctx, app: &mut App, to: i64) -> i64 {
    press(app, "Tab");
    press(app, "Tab");
    for _ in 0..40 {
        let now = alpha_now(app);
        if now <= to {
            break;
        }
        let chord = if now - to >= 10 {
            "Shift+ArrowLeft"
        } else {
            "ArrowLeft"
        };
        press(app, chord);
        if !ctx.until("the alpha to move", 1_500, &mut || alpha_now(app) != now) {
            break;
        }
    }
    alpha_now(app)
}
/// The format toggle in the popover, clicked.
fn click_format(app: &mut App) -> bool {
    let parts = state(app)["parts"]["format"].clone();
    click_r(app, &parts)
}

/// Types `text` into the code box of a colour row and presses Enter.
fn type_code(app: &mut App, id: &str, text: &str) {
    let r = row(app, id);
    click_r(app, &r["code_box"]);
    press(app, crate::chord::FIELD_SELECT_ALL);
    let _ = app.session.type_text(text);
    press(app, "Enter");
}

/// Types a line into the focused shell and presses Enter (after its prompt shows).
fn type_line(app: &mut App, line: &str) {
    let front = app
        .session
        .state()
        .ok()
        .and_then(|s| s.front)
        .unwrap_or_default();
    let _ = wait_for_prompt(
        app,
        &front,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    );
    let _ = app.session.type_text(line);
    let _ = app.session.key("Enter");
}

/// PT's `hookSay`, for the flare-agent protocol: a PowerShell line that writes one OSC 777 hook.
fn say(state: &str) -> String {
    format!("[Console]::Write([char]27 + ']777;flare-agent;state={state}' + [char]7)")
}

/// The state of the first tab's mark.
fn mark_state(app: &mut App) -> String {
    tabs_of(app)
        .first()
        .and_then(|t| t["mark"]["state"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// A decoded snapshot: device pixels, row-major.
struct Snap {
    w: u32,
    h: u32,
    px: Vec<Rgb>,
}

impl Snap {
    fn at(&self, x: u32, y: u32) -> Rgb {
        self.px[(y.min(self.h - 1) * self.w + x.min(self.w - 1)) as usize]
    }
}

fn read_png(path: &Path) -> Option<Snap> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .ok()?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader.next_frame(&mut buf).ok()?;
    let step = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        _ => 1,
    };
    let px: Vec<Rgb> = buf[..info.buffer_size()]
        .chunks_exact(step)
        .map(|c| match step {
            1 | 2 => [c[0], c[0], c[0]],
            _ => [c[0], c[1], c[2]],
        })
        .collect();
    (px.len() as u32 == info.width * info.height).then_some(Snap {
        w: info.width,
        h: info.height,
        px,
    })
}

fn dist(a: Rgb, b: Rgb) -> i32 {
    (0..3)
        .map(|i| (i32::from(a[i]) - i32::from(b[i])).abs())
        .sum()
}

/// The ground and the ink of the terminal row whose text satisfies `want`: the ground is the pixel
/// at the grid's right edge, the ink the pixel in the row farthest from it. None when no row
/// matches or the snapshot fails.
fn row_ink(
    ctx: &mut Ctx,
    app: &mut App,
    name: &str,
    want: &dyn Fn(&str) -> bool,
) -> Option<(Rgb, Rgb)> {
    let grid = rect_of(&probe(app, "ui.rect", json!({ "name": "grid" })))?;
    let text = probe(app, "grid.text", Value::Null);
    let rows = text["rows"].as_array()?.clone();
    let rows_n = text["rows_n"].as_f64()?;
    let index = rows.iter().position(|r| want(r.as_str().unwrap_or("")))?;
    let k = scale_of(app);
    let cell_h = grid.3 / rows_n;
    let png = ctx.shot(app, name)?;
    let s = read_png(&png)?;
    let y0 = ((grid.1 + index as f64 * cell_h) * k).round() as u32;
    let y1 = ((grid.1 + (index as f64 + 1.0) * cell_h) * k).round() as u32;
    let x0 = (grid.0 * k).round() as u32;
    let x1 = (((grid.0 + grid.2) * k).round() as u32).min(s.w);
    let ground = s.at(x1.saturating_sub(3), (y0 + y1) / 2);
    let mut best = ground;
    for y in y0..y1.min(s.h) {
        for x in x0..x1 {
            let c = s.at(x, y);
            if dist(c, ground) > dist(best, ground) {
                best = c;
            }
        }
    }
    Some((ground, best))
}

fn hex_rgb(hex: &str) -> Rgb {
    let h = hex.trim_start_matches('#');
    let ch = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    [ch(0), ch(2), ch(4)]
}

fn alpha_of_hex8(hex: &str) -> f64 {
    let h = hex.trim_start_matches('#');
    f64::from(u8::from_str_radix(h.get(6..8).unwrap_or("ff"), 16).unwrap_or(255)) / 255.0
}

/// The scenario's tab and Settings launch: one tab on the alpha folder, a window big enough that a
/// page shows many rows.
fn launch_settings(
    ctx: &mut Ctx,
    extra_args: &[&str],
    env: &[(&str, &str)],
    scale: Option<u32>,
) -> Result<App, String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let mut app = ctx.launch(Launch {
        args: extra_args.iter().map(|s| (*s).to_string()).collect(),
        env: env
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
        folders: vec![alpha],
        renderer: None,
        scale_dpi: scale,
        ..Launch::default()
    })?;
    ctx.until("a tab open", 15_000, &mut || !tabs_of(&mut app).is_empty());
    resize(&mut app, 1600.0, 1000.0);
    Ok(app)
}

// ---- colourPicker ----------------------------------------------------------------------------------------

/// Brings a wall card into view and clicks it; true when it is the theme in force afterwards.
fn pick_card(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    let card_of = |app: &mut App| -> Option<Value> {
        probe(app, "picker.wall", Value::Null)["cards"]
            .as_array()?
            .iter()
            .find(|c| c["id"] == json!(id))
            .cloned()
    };
    // Open the wall so every card is a target.
    let wall = probe(app, "picker.wall", Value::Null);
    if wall["expanded"] == json!(false) {
        bring_into_view(ctx, app, "the wall's toggle", &mut |a| {
            let w = probe(a, "picker.wall", Value::Null);
            Some((
                w["toggle"]["y"].as_f64()?,
                w["toggle_visible"] == json!(true),
            ))
        });
        let toggle = probe(app, "picker.wall", Value::Null)["toggle"].clone();
        click_r(app, &toggle);
        ctx.until("the wall to open", 4_000, &mut || {
            probe(app, "picker.wall", Value::Null)["expanded"] == json!(true)
        });
    }
    if !bring_into_view(ctx, app, &format!("the {id} card"), &mut |a| {
        let c = card_of(a)?;
        Some((c["rect"]["y"].as_f64()?, c["visible"] == json!(true)))
    }) {
        return false;
    }
    let Some(card) = card_of(app) else {
        return false;
    };
    click_r(app, &card["rect"]);
    // A pick with changes unsaved opens the question instead of switching, so that is an answer
    // too: the wait ends on either, records nothing, and the caller's check says which it wanted.
    ctx.poll(4_000, &mut || {
        card_of(app).is_some_and(|c| c["on"] == json!(true))
            || state(app)["ask_open"] == json!(true)
    });
    card_of(app).is_some_and(|c| c["on"] == json!(true))
}

/// Opens the theme editor on the card in force by its pencil.
fn open_editor(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    let card = probe(app, "picker.wall", Value::Null)["cards"]
        .as_array()
        .and_then(|c| c.iter().find(|c| c["id"] == json!(id)).cloned());
    let Some(card) = card else {
        return false;
    };
    click_r(app, &card["pencil"]);
    ctx.until("the theme editor to open", 4_000, &mut || {
        probe(app, "picker.editor", Value::Null)["open"] == json!(true)
    })
}

fn editor(app: &mut App) -> Value {
    probe(app, "picker.editor", Value::Null)
}

/// The editor's well with this label (any case).
fn well(app: &mut App, label: &str) -> Option<Value> {
    editor(app)["wells"]
        .as_array()?
        .iter()
        .find(|w| str_of(w, "label").eq_ignore_ascii_case(label))
        .cloned()
}

/// Opens a well's popover, walks its alpha to `to`, then a press on the editor's own title keeps the
/// colour and closes the popover. Returns the alpha it stopped at.
fn walk_well(ctx: &mut Ctx, app: &mut App, label: &str, to: i64) -> Option<i64> {
    let w = well(app, label)?;
    click_r(app, &w["swatch"]);
    if !wait_picker(ctx, app, true, &format!("the {label} picker to open")) {
        return None;
    }
    let got = walk_alpha(ctx, app, to);
    press(app, "Tab"); // stays inside, keeps the colour
                       // A press outside the popover, on the editor's own title (not its scrim, which cancels the
                       // editor), keeps the colour and closes it.
    let b = rect_of(&editor(app)["box"]).unwrap_or((400.0, 200.0, 600.0, 400.0));
    click_at(app, ((b.0 + 60.0) as f32, (b.1 + 30.0) as f32));
    wait_picker(ctx, app, false, &format!("the {label} picker to close"));
    Some(got)
}

fn custom_json(app: &App) -> Value {
    pref(app, "flare.term.custom")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

/// The contrast of a (foreground, background) pair of `#rrggbbaa` strings, and the foreground's alpha.
fn pair_contrast(fg: &str, bg: &str) -> (f64, f64) {
    (contrast(hex_rgb(fg), hex_rgb(bg)), alpha_of_hex8(fg))
}

/// PT: run.mjs:6014-6288. The agent colour's picker, the format toggle, the typed code, the
/// eyedropper and the two layers that put it away; then the theme editor's alphas, saved as Custom.
pub fn colour_picker(ctx: &mut Ctx) -> Result<(), String> {
    let profile = ctx.ensure_world().profile.clone();
    seed_prefs(&profile, &[("flare.term.agentIndicator", "minimal")])?;
    let mut app = launch_settings(
        ctx,
        &["--preview-update"],
        &[
            ("FLARE_E2E_REDUCED_MOTION", "1"),
            ("FLARE_E2E_PICK_COLOUR", "#ff0000"),
        ],
        Some(DPI_225),
    )?;
    const ID: &str = "agent-color";
    const KEY: &str = "flare.term.agentColor";

    // A shell stands in for Claude, mid-answer, as in `indicator`. macOS (rule 339): pwsh on the runner
    // gives its prompt long after the process poll's first "no agent" for the tab, which takes a bare
    // hook claim back at once, so the Mac run starts the agent stand-in in the tab first, as agentHooks
    // and backgroundWork do (the hook then belongs to a real agent process).
    #[cfg(target_os = "macos")]
    {
        let cmd = crate::standins::agent_proc::command(&app.profile)?;
        type_line(&mut app, &cmd);
        std::thread::sleep(std::time::Duration::from_millis(
            crate::standins::agent_proc::SETTLE_MS,
        )); // sync-ok: test harness, off the app
    }
    type_line(&mut app, &say("working"));
    let working = ctx.until("the stand-in agent to work", 12_000, &mut || {
        mark_state(&mut app) == "working"
    });
    // PT: run.mjs:6045
    ctx.ok(working, "a working stand-in agent is on the strip");

    let opened_settings = open_settings(ctx, &mut app);
    if !opened_settings || !goto_page(ctx, &mut app, "agents") {
        ctx.ok(false, "the cog opens Settings on the Agents page");
        app.close();
        return Ok(());
    }
    reveal(ctx, &mut app, ID);
    let r0 = row(&mut app, ID);
    let prefix = state(&mut app)["names"]["swatch_prefix"]
        .as_str()
        .unwrap_or("")
        .to_string();
    // PT: run.mjs:6052
    let follows = settled_pref(ctx, &app, KEY).is_none();
    ctx.ok(
        follows,
        "the working colour follows the theme to begin with",
    );
    // PT: run.mjs:6053. The swatch's accessible name is "Pick " plus the row's label (the literal in
    // colour.slint, returned by the probe).
    let named = format!("{prefix}{}", str_of(&r0, "label"));
    ctx.ok(
        named == "Pick Agent working colour",
        format!("the row has a swatch named for it ({named})"),
    );
    // DROPPED: "and no native colour input is left on the page" (PT: run.mjs:6054) because Slint
    // draws every control itself; there is no native colour input to leave behind.

    // Opened and shut with no change: nothing stored, no Reset.
    let opened = open_swatch(ctx, &mut app, ID);
    // PT: run.mjs:6058
    ctx.ok(opened, "the swatch opens the picker");
    let st = state(&mut app);
    // PT: run.mjs:6059
    ctx.ok(
        str_of(&st, "label") == "Agent working colour",
        format!("named for the row's colour ({})", str_of(&st, "label")),
    );
    let names = format!(
        "{}, {}, {}",
        str_of(&st["names"], "sv"),
        str_of(&st["names"], "hue"),
        str_of(&st["names"], "alpha")
    );
    // PT: run.mjs:6061
    ctx.ok(
        names == "Saturation and brightness, Hue, Alpha" && !st["parts"]["alpha"].is_null(),
        format!("with its three sliders ({names})"),
    );
    // PT: run.mjs:6062
    ctx.ok(
        str_of(&st, "format") == "HEX",
        format!("and the format reads HEX ({})", str_of(&st, "format")),
    );
    ctx.shot(&mut app, "open");
    let closed = escape_picker(ctx, &mut app);
    // PT: run.mjs:6064
    ctx.ok(closed, "Escape closes it");
    let nothing =
        settled_pref(ctx, &app, KEY).is_none() && row(&mut app, ID)["reset"] == json!(false);
    // PT: run.mjs:6065
    ctx.ok(
        nothing,
        "an open and close with no change stores nothing and shows no Reset",
    );
    // PT: run.mjs:6066. Focus is not queryable, so the swatch having it is proved by what a key does
    // there: Space on the focused swatch opens its picker again.

    press(&mut app, "Space");
    let back = wait_picker(
        ctx,
        &mut app,
        true,
        "Space on the swatch to open the picker",
    );
    ctx.ok(back, "and the focus is back on the swatch");
    escape_picker(ctx, &mut app);

    // Half alpha, on the keyboard.
    open_swatch(ctx, &mut app, ID);
    let got = walk_alpha(ctx, &mut app, 50);
    // PT: run.mjs:6079
    ctx.ok(
        got == 50,
        format!("Shift+Left walks the alpha to 50 percent ({got})"),
    );
    pref_becomes(ctx, &app, KEY, "the working colour to be stored", &|v| {
        v.is_some_and(is_hex8)
    });
    let stored = pref(&app, KEY).unwrap_or_default();
    // PT: run.mjs:6085
    ctx.ok(
        is_hex8(&stored) && stored.ends_with("80"),
        format!("the working colour is stored as hex8 ({stored})"),
    );
    let code = str_of(&row(&mut app, ID), "code").to_string();
    // PT: run.mjs:6086
    ctx.ok(
        norm_code(&code).len() == 8 && norm_code(&code).chars().all(|c| c.is_ascii_hexdigit()),
        format!("and the code field shows the eight digits ({code})"),
    );
    // A SEE-THROUGH WORKING COLOUR IS DRAWN OPAQUE (#112, #143): the run is the colour laid on the
    // ground, never a see-through bar.
    let opaque = ctx.until(
        "the working mark to wear an opaque colour",
        4_000,
        &mut || probe(&mut app, "agent.working", Value::Null)["alpha"] == json!(255),
    );
    let mark = probe(&mut app, "agent.working", Value::Null);
    // PT: run.mjs:6094
    ctx.ok(
        opaque,
        format!("the working line is opaque ({})", str_of(&mark, "colour")),
    );
    ctx.shot(&mut app, "alpha");

    // Escape after a write: the row follows the theme again.
    let closed = escape_picker(ctx, &mut app);
    // PT: run.mjs:6102
    ctx.ok(closed, "Escape closes the changed picker");
    let followed = pref_becomes(ctx, &app, KEY, "the row to follow the theme again", &|v| {
        v.is_none()
    });
    // PT: run.mjs:6103
    ctx.ok(followed, "and puts back a row that follows the theme");
    // PT: run.mjs:6104
    ctx.ok(
        row(&mut app, ID)["reset"] == json!(false),
        "with no Reset showing",
    );

    // The format toggle, and a typed hsla().
    open_swatch(ctx, &mut app, ID);
    click_format(&mut app);
    ctx.until("the format to read RGBA", 3_000, &mut || {
        str_of(&state(&mut app), "format") == "RGBA"
    });
    let rgba_code = str_of(&row(&mut app, ID), "code").to_string();
    let rgba_ok = str_of(&state(&mut app), "format") == "RGBA"
        && rgba_code.starts_with("rgba(")
        && rgba_code.ends_with(", 1)");
    // PT: run.mjs:6109
    ctx.ok(rgba_ok, format!("RGBA shows rgba() ({rgba_code})"));
    click_format(&mut app);
    ctx.until("the format to read HSLA", 3_000, &mut || {
        str_of(&state(&mut app), "format") == "HSLA"
    });
    let hsla_code = str_of(&row(&mut app, ID), "code").to_string();
    let hsla_ok = str_of(&state(&mut app), "format") == "HSLA"
        && hsla_code.starts_with("hsla(")
        && hsla_code.ends_with(", 1)");
    // PT: run.mjs:6111
    ctx.ok(hsla_ok, format!("HSLA shows hsla() ({hsla_code})"));
    let remembered = pref_becomes(
        ctx,
        &app,
        "flare.term.colourFormat",
        "the format to be stored",
        &|v| v == Some("hsla"),
    );
    // PT: run.mjs:6113
    ctx.ok(
        remembered && pref(&app, KEY).is_none(),
        "the toggle is remembered and stores no colour",
    );
    ctx.shot(&mut app, "hsla");
    escape_picker(ctx, &mut app);
    type_code(&mut app, ID, "hsla(200, 50%, 40%, 0.5)");
    let typed = pref_becomes(ctx, &app, KEY, "the typed hsla() to be stored", &|v| {
        v.is_some_and(|s| s.eq_ignore_ascii_case("#33779980"))
    });
    // FLARE-DEFECT: colourPicker / a typed hsla() stores the same hex8 / crates/flare/src/appctl.rs
    // (`on_key`, the NotAChord arm consumes a letter when no shell is in front, so typing never
    // reaches the Settings code field) and crates/flare/ui/app.slint (`chrome-has-focus`).
    // PT: run.mjs:6118
    ctx.ok(
        typed,
        format!(
            "a typed hsla() stores the same hex8 ({:?})",
            pref(&app, KEY)
        ),
    );
    // PT: run.mjs:6119
    ctx.ok(
        row(&mut app, ID)["reset"] == json!(true),
        "and the row offers Reset",
    );
    open_swatch(ctx, &mut app, ID);
    click_format(&mut app);
    ctx.until("the format to read HEX", 3_000, &mut || {
        str_of(&state(&mut app), "format") == "HEX"
    });
    let hex_code = str_of(&row(&mut app, ID), "code").to_string();
    // PT: run.mjs:6124
    ctx.ok(
        str_of(&state(&mut app), "format") == "HEX" && norm_code(&hex_code) == "33779980",
        format!("back to HEX, the field is the stored form ({hex_code})"),
    );

    // The eyedropper, stubbed by FLARE_E2E_PICK_COLOUR: the screen's RGB, the colour's own alpha.
    escape_picker(ctx, &mut app);
    open_swatch(ctx, &mut app, ID);
    let dropper_on = state(&mut app)["dropper_on"] == json!(true);
    // PT: run.mjs:6139
    ctx.ok(
        dropper_on,
        "the eyedropper is offered where the window has one",
    );
    let drop = state(&mut app)["parts"]["dropper"].clone();
    click_r(&mut app, &drop);
    let dropped = pref_becomes(ctx, &app, KEY, "the dropped colour to be stored", &|v| {
        v.is_some_and(|s| s.eq_ignore_ascii_case("#ff000080"))
    });
    // PT: run.mjs:6142
    ctx.ok(
        dropped,
        format!("a dropped colour keeps the alpha ({:?})", pref(&app, KEY)),
    );
    escape_picker(ctx, &mut app);
    let back = pref_becomes(ctx, &app, KEY, "Escape to take the drop back", &|v| {
        v.is_some_and(|s| s.eq_ignore_ascii_case("#33779980"))
    });
    // PT: run.mjs:6145
    ctx.ok(
        back,
        "and Escape takes the drop back to what the picker opened with",
    );

    // Command help and the update window each put the picker away.
    open_swatch(ctx, &mut app, ID);
    press(&mut app, "F1");
    let help = ctx.until("Command help to open", 4_000, &mut || {
        probe(&mut app, "picker.layers", Value::Null)["help_open"] == json!(true)
    });
    // PT: run.mjs:6152
    ctx.ok(help, "F1 opens Command help over the picker");
    // PT: run.mjs:6153
    let gone = wait_picker(ctx, &mut app, false, "the picker to go under help");
    ctx.ok(gone, "and the picker is gone");
    press(&mut app, "Escape");
    let shut = ctx.until("Escape to close Command help", 4_000, &mut || {
        probe(&mut app, "picker.layers", Value::Null)["help_open"] == json!(false)
    });
    ctx.ok(
        shut,
        "Escape closes Command help opened over the Settings page",
    );
    open_swatch(ctx, &mut app, ID);
    // Where PT clicks the chip with no pointer press, Flare's chip answers a click, which is also a
    // press outside the picker; either way the picker must be gone and the window open.
    ctx.until("the update chip to show", 8_000, &mut || {
        probe(&mut app, "picker.layers", Value::Null)["update_chip"]["shown"] == json!(true)
    });
    let chip = probe(&mut app, "picker.layers", Value::Null)["update_chip"].clone();
    // The open popover has a window-wide catcher under it, so the first press on the chip is a press
    // outside the picker (it closes, keeping the colour) and the second opens the update window.
    click_r(&mut app, &chip);
    wait_picker(
        ctx,
        &mut app,
        false,
        "the picker to close under the chip's press",
    );
    click_r(&mut app, &chip);
    let update = ctx.until("the update window to open", 4_000, &mut || {
        probe(&mut app, "picker.layers", Value::Null)["update_open"] == json!(true)
    });
    // PT: run.mjs:6165
    ctx.ok(update, "the update window opens");
    // PT: run.mjs:6166
    let gone = wait_picker(
        ctx,
        &mut app,
        false,
        "the picker to go under the update window",
    );
    ctx.ok(gone, "and the picker is gone");
    // PT: run.mjs:6167
    ctx.ok(
        pref(&app, KEY).is_some_and(|s| s.eq_ignore_ascii_case("#33779980")),
        "neither layer changed the colour",
    );
    press(&mut app, "Escape");
    ctx.until("the update window to close", 4_000, &mut || {
        probe(&mut app, "picker.layers", Value::Null)["update_open"] == json!(false)
    });
    // The Reset link sits 10 px left of the code box.
    let code_box = row(&mut app, ID)["code_box"].clone();
    if let Some(b) = rect_of(&code_box) {
        click_at(
            &mut app,
            ((b.0 - 10.0 - 14.0) as f32, (b.1 + b.3 / 2.0) as f32),
        );
    }
    pref_becomes(ctx, &app, KEY, "Reset to follow the theme", &|v| {
        v.is_none()
    });

    // The theme editor: red at 40%, the text at 30%, saved as Custom.
    goto_page(ctx, &mut app, "appearance");
    pick_card(ctx, &mut app, "pitch");
    let editor_open = open_editor(ctx, &mut app, "pitch");
    let red = walk_well(ctx, &mut app, "red", 40);
    let fore = walk_well(ctx, &mut app, "Foreground", 30);
    // PT: run.mjs:6190
    ctx.ok(
        editor_open && red == Some(40) && fore == Some(30),
        format!("red walks to 40 percent and the text to 30 ({red:?}, {fore:?})"),
    );
    // The preview card draws what the terminal draws (#113 review): the 30 percent text composited
    // and floored to 4.5:1, not painted at 30.
    let preview = editor(&mut app)["preview"].clone();
    let (cr, ca) = pair_contrast(str_of(&preview, "fg"), str_of(&preview, "bg"));
    // PT: run.mjs:6207
    ctx.ok(
        ca == 1.0 && cr >= 4.5,
        format!(
            "the editor's preview draws the 30 percent text as the terminal does ({cr:.2}:1, {})",
            str_of(&preview, "fg")
        ),
    );
    // The Selection's cap is 254/255: never announced as 100 percent opaque. The cap is the
    // alpha bar's top: End walks to it.
    if let Some(w) = well(&mut app, "Selection") {
        click_r(&mut app, &w["swatch"]);
        wait_picker(ctx, &mut app, true, "the Selection picker to open");
    }
    press(&mut app, "Tab");
    press(&mut app, "Tab");
    press(&mut app, "End");
    ctx.until("the alpha to rest at its top", 2_000, &mut || {
        alpha_now(&mut app) >= 99
    });
    let top = alpha_now(&mut app);
    // PT: run.mjs:6217. Where PT reads aria-valuemax, End on the bar walks to the cap.
    ctx.ok(
        top == 99,
        format!("the Selection's alpha tops out at 99 percent, not 100 ({top})"),
    );
    escape_picker(ctx, &mut app);
    // The format toggle widens every code field, which moves the swatches in the editor's grid: the
    // open picker follows its own (#113 review).
    let beside = |app: &mut App| -> Option<(bool, f64, f64)> {
        let w = well(app, "Foreground")?;
        let sw = rect_of(&w["swatch"])?;
        let p = rect_of(&state(app)["parts"]["box"])?;
        let under =
            (p.1 - (sw.1 + sw.3 + 6.0)).abs() <= 1.5 || ((p.1 + p.3) - (sw.1 - 6.0)).abs() <= 1.5;
        Some((
            ((p.0 + p.2) - (sw.0 + sw.2)).abs() <= 1.5 && under,
            sw.0 + sw.2,
            p.0 + p.2,
        ))
    };
    if let Some(w) = well(&mut app, "Foreground") {
        click_r(&mut app, &w["swatch"]);
        wait_picker(ctx, &mut app, true, "the Foreground picker to open");
    }
    let at_hex = beside(&mut app);
    click_format(&mut app);
    ctx.until("the editor's fields to widen", 3_000, &mut || {
        str_of(&state(&mut app), "format") == "RGBA"
    });
    let at_rgba = beside(&mut app);
    let moved = matches!((at_hex, at_rgba), (Some(a), Some(b)) if (a.1 - b.1).abs() > 0.5);

    // PT: run.mjs:6232
    ctx.ok(
        at_hex.is_some_and(|a| a.0) && at_rgba.is_some_and(|b| b.0) && moved,
        format!(
            "the picker stays on its swatch as RGBA widens the fields ({at_hex:?} to {at_rgba:?})"
        ),
    );
    ctx.shot(&mut app, "editor-rgba");
    click_format(&mut app);
    click_format(&mut app);
    ctx.until("the format to be back to HEX", 3_000, &mut || {
        str_of(&state(&mut app), "format") == "HEX"
    });
    // PT: run.mjs:6236
    ctx.ok(
        str_of(&state(&mut app), "format") == "HEX",
        "and back to HEX",
    );
    escape_picker(ctx, &mut app);
    let bg_well = well(&mut app, "Background");

    // PT: run.mjs:6239
    ctx.ok(bg_well.is_some(), "the Background well is there");
    if let Some(w) = &bg_well {
        click_r(&mut app, &w["swatch"]);
        wait_picker(ctx, &mut app, true, "the Background picker to open");
    }
    // The theme's Background is a colour only (owner, 2026-10-13: the see-through slider "only").
    // PT: run.mjs:6244
    ctx.ok(
        state(&mut app)["alpha_on"] == json!(false),
        "the theme's Background has no Alpha",
    );
    escape_picker(ctx, &mut app);
    ctx.shot(&mut app, "editor");
    let save = editor(&mut app)["save"].clone();
    if let Some(s) = rect_of(&save) {
        click_at(&mut app, ((s.0 + 16.0) as f32, (s.1 + s.3 / 2.0) as f32));
    }
    ctx.until("the Custom theme to be saved", 4_000, &mut || {
        pref(&app, "flare.term.custom").is_some()
    });
    let custom = custom_json(&app);
    let red_c = custom["ansi"]["red"].as_str().unwrap_or("").to_string();
    let fg_c = custom["fg"].as_str().unwrap_or("").to_string();
    // PT: run.mjs:6252
    ctx.ok(
        red_c.len() == 9 && red_c.ends_with("66") && fg_c.len() == 9 && fg_c.ends_with("4d"),
        format!("the saved Custom keeps both alphas (red {red_c}, text {fg_c})"),
    );
    let wall_card = probe(&mut app, "picker.wall", Value::Null)["cards"]
        .as_array()
        .and_then(|c| c.iter().find(|c| c["id"] == json!("custom")).cloned());
    let (cr2, ca2) = wall_card
        .as_ref()
        .map(|c| pair_contrast(str_of(c, "fg"), str_of(c, "bg")))
        .unwrap_or((0.0, 0.0));
    // PT: run.mjs:6255
    ctx.ok(
        wall_card.is_some() && ca2 == 1.0 && cr2 >= 4.5,
        format!("the wall's Custom card draws the text as the terminal does ({cr2:.2}:1)"),
    );

    // The terminal wears it: a dark red line and the 30 percent text, measured off the snapshot.
    if let Some(t) = tabs_of(&mut app).first().and_then(|t| rect_of(&t["rect"])) {
        click_at(&mut app, centre(t));
    }
    type_line(&mut app, "cls; Write-Host REDTEXT -ForegroundColor DarkRed");
    let seen = ctx.until("the red line to show", 8_000, &mut || {
        probe(&mut app, "grid.text", Value::Null)["rows"]
            .as_array()
            .is_some_and(|r| {
                r.iter()
                    .any(|l| l.as_str().is_some_and(|s| s.trim() == "REDTEXT"))
            })
    });
    // PT: run.mjs:6279
    ctx.ok(seen, "the red line is on screen");
    if seen {
        let red_row = row_ink(ctx, &mut app, "redtext", &|l| l.trim() == "REDTEXT");
        let text_row = row_ink(ctx, &mut app, "prompt", &|l| {
            l.trim_start().starts_with("PS ")
        });
        let red_ratio = red_row.map_or(0.0, |(g, i)| contrast(i, g));
        // PT: run.mjs:6281
        ctx.ok(
            red_ratio >= 3.0,
            format!(
                "the applied red reads at 3:1 on the ground ({red_ratio:.2}:1, {:?})",
                red_row.map(|r| r.1)
            ),
        );
        let text_ratio = text_row.map_or(0.0, |(g, i)| contrast(i, g));
        // PT: run.mjs:6282
        ctx.ok(
            text_ratio >= 4.5,
            format!(
                "and the 30 percent text at 4.5:1 ({text_ratio:.2}:1, {:?})",
                text_row.map(|r| r.1)
            ),
        );
    }
    open_settings(ctx, &mut app);
    goto_page(ctx, &mut app, "appearance");
    bring_into_view(ctx, &mut app, "the Custom card", &mut |a| {
        let w = probe(a, "picker.wall", Value::Null);
        let c = w["cards"]
            .as_array()?
            .iter()
            .find(|c| c["id"] == json!("custom"))?
            .clone();
        Some((c["rect"]["y"].as_f64()?, c["visible"] == json!(true)))
    });
    let reopened = open_editor(ctx, &mut app, "custom");
    let shown = |app: &mut App, label: &str| -> String {
        well(app, label)
            .map(|w| str_of(&w, "text").to_string())
            .unwrap_or_default()
    };
    let (sr, sf) = (shown(&mut app, "red"), shown(&mut app, "Foreground"));
    // PT: run.mjs:6291
    ctx.ok(
        reopened && sr.to_lowercase().ends_with("66") && sf.to_lowercase().ends_with("4d"),
        format!("reopening Custom shows the 40 and 30 percent alphas ({sr}, {sf})"),
    );
    let cancel = editor(&mut app)["cancel"].clone();
    if let Some(c) = rect_of(&cancel) {
        click_at(&mut app, ((c.0 + 14.0) as f32, (c.1 + c.3 / 2.0) as f32));
    }
    ctx.until("the editor to close", 3_000, &mut || {
        editor(&mut app)["open"] == json!(false)
    });
    // Save changes: lit by an agent colour, it keeps the palette's alphas.
    goto_page(ctx, &mut app, "agents");
    reveal(ctx, &mut app, ID);
    type_code(&mut app, ID, "#3da9fc");
    pref_becomes(ctx, &app, KEY, "the agent colour to be stored", &|v| {
        v.is_some()
    });
    // The Save sits in the Mark colours section of the Agents page, where the lit colour is.
    bring_into_view(ctx, &mut app, "the Save button", &mut |a| {
        let s = probe(a, "picker.save", Value::Null);
        Some((s["point"]["y"].as_f64()?, s["visible"] == json!(true)))
    });
    let sv = probe(&mut app, "picker.save", Value::Null);
    if let (Some(x), Some(y)) = (sv["point"]["x"].as_f64(), sv["point"]["y"].as_f64()) {
        click_at(&mut app, (x as f32, y as f32));
    }
    ctx.settle(50);
    let again = {
        hold(ctx, 450);
        custom_json(&app)
    };
    // PT: run.mjs:6303
    ctx.ok(
        again["ansi"]["red"] == custom["ansi"]["red"] && again["fg"] == custom["fg"],
        format!(
            "Save changes keeps them (red {}, text {})",
            again["ansi"]["red"], again["fg"]
        ),
    );
    // End idle, so the close is not held on the agent question.
    if let Some(t) = tabs_of(&mut app).first().and_then(|t| rect_of(&t["rect"])) {
        click_at(&mut app, centre(t));
    }
    type_line(&mut app, &say("done"));
    ctx.until("the stand-in agent to stop working", 8_000, &mut || {
        mark_state(&mut app) != "working"
    });
    app.close();
    Ok(())
}

// ---- pickedGround ----------------------------------------------------------------------------------------

/// PT: run.mjs:6289-6349. On the dark default theme, a light Background colour: the prompt's text is
/// measured against it, off the snapshot. And with an Accent colour picked, the Agent working
/// indicator's field shows that accent, not the theme's.
pub fn picked_ground(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = launch_settings(
        ctx,
        &[],
        &[("FLARE_E2E_REDUCED_MOTION", "1")],
        Some(DPI_225),
    )?;
    let front = app
        .session
        .state()
        .ok()
        .and_then(|s| s.front)
        .unwrap_or_default();
    wait_for_prompt(
        &mut app,
        &front,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    );
    type_line(&mut app, "cls");
    ctx.until("the screen to clear", 6_000, &mut || {
        probe(&mut app, "grid.text", Value::Null)["rows"]
            .as_array()
            .is_some_and(|r| {
                r.iter()
                    .filter(|l| l.as_str().is_some_and(|s| !s.trim().is_empty()))
                    .count()
                    <= 1
            })
    });
    let before = row_ink(ctx, &mut app, "before", &|l| {
        l.trim_start().starts_with("PS ")
    });
    let b = before.map_or(0.0, |(g, i)| contrast(i, g));
    // PT: run.mjs:6310
    ctx.ok(
        b >= 4.5,
        format!("the theme's text reads on its own ground ({b:.1}:1)"),
    );
    // Through the setting's own field, so the store's listeners fire.
    if !open_settings(ctx, &mut app) || !goto_page(ctx, &mut app, "appearance") {
        ctx.ok(false, "the cog opens the Appearance page");
        app.close();
        return Ok(());
    }
    reveal(ctx, &mut app, "window-background");
    type_code(&mut app, "window-background", "#F4F1E8");
    reveal(ctx, &mut app, "window-accent");
    type_code(&mut app, "window-accent", "#1D3FBF");
    goto_page(ctx, &mut app, "agents");
    reveal(ctx, &mut app, "agent-color");
    let working = ctx.until(
        "the working indicator to follow the accent",
        3_000,
        &mut || norm_code(str_of(&row(&mut app, "agent-color"), "code")).starts_with("1d3fbf"),
    );
    let code = str_of(&row(&mut app, "agent-color"), "code").to_string();
    // FLARE-DEFECT: pickedGround / the working indicator follows the picked accent / crates/flare/src/appctl.rs
    // (`on_key`, the NotAChord arm: typing into a Settings code field is consumed with no shell in
    // front, so the picked accent never arrives).
    // PT: run.mjs:6325
    ctx.ok(
        working,
        format!("the working indicator follows the picked accent ({code})"),
    );
    press(&mut app, "Ctrl+Tab"); // back to the shell
    if let Some(t) = tabs_of(&mut app).first().and_then(|t| rect_of(&t["rect"])) {
        click_at(&mut app, centre(t));
    }
    let landed = ctx.until("the ground to take the picked colour", 4_000, &mut || {
        probe(&mut app, "window.ground", Value::Null)["ground"] == json!("#f4f1e8")
    });
    let after = if landed {
        row_ink(ctx, &mut app, "after", &|l| {
            l.trim_start().starts_with("PS ")
        })
    } else {
        None
    };
    let a = after.map_or(0.0, |(g, i)| contrast(i, g));
    // PT: run.mjs:6330
    ctx.ok(
        landed && a >= 4.5,
        format!(
            "on a picked light background the text still reads ({a:.1}:1, ink {:?})",
            after.map(|r| r.1)
        ),
    );
    app.close();
    Ok(())
}

// ---- opacityAlpha ----------------------------------------------------------------------------------------

/// Sets a slider row to `value` by a press on its track (9 px is the bottom of the travel, 159 px
/// the top), then checks it read back. True when the row reads `value`.
fn set_slider(ctx: &mut Ctx, app: &mut App, id: &str, value: i64) -> bool {
    if !reveal(ctx, app, id) {
        return false;
    }
    let r = row(app, id);
    let max = r["num_max"].as_f64().unwrap_or(70.0);
    if let Some(c) = rect_of(&r["control"]) {
        let x = c.0 + 9.0 + 150.0 * (value as f64 / max);
        click_at(app, (x as f32, (c.1 + c.3 / 2.0) as f32));
    }
    ctx.until(&format!("{id} to read {value}"), 3_000, &mut || {
        row(app, id)["num"].as_i64() == Some(value)
    })
}

fn slider_num(app: &mut App, id: &str) -> i64 {
    row(app, id)["num"].as_i64().unwrap_or(-1)
}

/// The ground token's hex8 now (`--p-bg`, as the Slint theme holds it).
fn ground_now(app: &mut App) -> String {
    str_of(&probe(app, "glass.paint", Value::Null), "ground").to_string()
}

fn open_appearance(ctx: &mut Ctx, app: &mut App) -> bool {
    open_settings(ctx, app) && goto_page(ctx, app, "appearance")
}

/// Answers the unsaved-changes question by its keys: the focus starts on Save as Custom, Shift+Tab
/// walks to Discard, Escape cancels. `answer` is "discard", "save" or "cancel".
fn answer_ask(ctx: &mut Ctx, app: &mut App, answer: &str) {
    match answer {
        "discard" => {
            press(app, "Shift+Tab");
            press(app, "Enter");
        }
        "save" => {
            press(app, "Enter");
        }
        _ => {
            press(app, "Escape");
        }
    }
    ctx.until("the question to close", 4_000, &mut || {
        state(app)["ask_open"] == json!(false)
    });
}

fn ask_open(ctx: &mut Ctx, app: &mut App, ms: u64) -> bool {
    // A miss is an answer ("no question opened"), so the wait records nothing.
    ctx.poll(ms, &mut || state(app)["ask_open"] == json!(true))
}

/// Relaunches on the same profile after seeding the old app's storage in a stand-in profile
/// (the import seam), the way PT seeds localStorage and quits. `entries` name the settings by
/// their short name (`theme`, `acrylic`, `opacity`, `custom`).
fn relaunch_with(
    ctx: &mut Ctx,
    app: &mut App,
    entries: &[(&str, &str)],
    scale: Option<u32>,
) -> Result<App, String> {
    let gone = app.close();
    // PT: run.mjs:6366
    ctx.ok(gone, "the app quits before it is launched again");
    let (roaming, profile) = {
        let w = ctx.ensure_world();
        (w.sandbox().roaming.clone(), w.profile.clone())
    };
    // The once-only import runs again on a profile that has not done it.
    let _ = std::fs::remove_file(profile.join("settings.json"));
    flare_settings::import::ldb::write_old_app_profile(&roaming, entries)?;
    let dir = roaming.to_string_lossy().into_owned();
    launch_settings(
        ctx,
        &[],
        &[
            ("FLARE_E2E_REDUCED_MOTION", "1"),
            ("FLARE_E2E_IMPORT_APPDATA", dir.as_str()),
        ],
        scale,
    )
}

/// PT: run.mjs:6350-6459. What the old app left, the window it was: the Opacity slider became the
/// Background's alpha (#114) and that alpha and the switch became the see-through slider. Old
/// storage is seeded as a stand-in profile, the app is launched on it (the import seam), and
/// the window must be the window it was. On the software renderer the ground is solid and the slider
/// is disabled (DIFF-5), so what paints or moves the slider is `skip`ped there.
pub fn opacity_alpha(ctx: &mut Ctx) -> Result<(), String> {
    // The first run is an empty profile (PT's first launch); it quits before the old storage is seeded.
    let mut app = launch_settings(ctx, &[], &[("FLARE_E2E_REDUCED_MOTION", "1")], None)?;
    let seed1 = [("theme", "dracula"), ("acrylic", "1"), ("opacity", "60")];
    // 1. A preset, Opacity 60, acrylic on.
    let mut app = relaunch_with(ctx, &mut app, &seed1, None)?;
    let gpu = on_gpu(&mut app);
    let solid = str_of(&probe(&mut app, "glass.paint", Value::Null), "solid").to_string();
    let ground_ok = if gpu {
        ctx.until("the ground to paint at alpha 0x99", 10_000, &mut || {
            let g = ground_now(&mut app);
            is_hex8(&g) && g.ends_with("99")
        })
    } else {
        false
    };
    if gpu {
        // PT: run.mjs:6371
        ctx.ok(
            ground_ok,
            format!(
                "a saved Opacity 60 paints the ground at alpha 0x99, as it did ({})",
                ground_now(&mut app)
            ),
        );
    } else {
        ctx.skip(SOFTWARE_SKIP);
    }
    let see = pref_becomes(
        ctx,
        &app,
        "flare.term.seeThrough",
        "the see-through level to be stored",
        &|v| v.is_some(),
    );
    let level = pref(&app, "flare.term.seeThrough");
    // PT: run.mjs:6377
    ctx.ok(
        see && level.as_deref() == Some("40"),
        format!("it now lives on the slider, at 40 ({level:?})"),
    );
    let picked = pref(&app, "flare.window.background");
    // PT: run.mjs:6379
    ctx.ok(
        picked
            .as_deref()
            .is_some_and(|p| p.len() == 7 && p.eq_ignore_ascii_case(&solid[..7.min(solid.len())])),
        format!("the Background it was carried on is opaque ({picked:?})"),
    );
    let keys = prefs_in(&app.profile);
    // PT: run.mjs:6381
    ctx.ok(
        !keys.contains_key("flare.term.opacity") && !keys.contains_key("flare.term.acrylic"),
        "and the old keys are gone",
    );
    let opened = open_appearance(ctx, &mut app);
    let ids: Vec<String> = probe(&mut app, "settings.rows", json!({}))["rows"]
        .as_array()
        .map(|r| r.iter().map(|x| str_of(x, "id").to_string()).collect())
        .unwrap_or_default();
    // PT: run.mjs:6386
    ctx.ok(
        opened && !ids.iter().any(|i| i == "term-opacity"),
        "there is no Opacity row",
    );
    let slider_off = row(&mut app, "term-acrylic")["off"] == json!(true);
    if !gpu || slider_off {
        // Everything below moves the slider or reads the painted ground, and so does the second
        // launch: none of it can run here.
        ctx.skip(SOFTWARE_SKIP);
        app.close();
        return Ok(());
    }
    reveal(ctx, &mut app, "term-acrylic");
    let n = slider_num(&mut app, "term-acrylic");
    // PT: run.mjs:6388
    ctx.ok(n == 40, format!("the slider reads 40 ({n})"));
    ctx.shot(&mut app, "row");
    // The Background is a colour only now.
    open_swatch(ctx, &mut app, "window-background");
    // PT: run.mjs:6397
    ctx.ok(
        state(&mut app)["alpha_on"] == json!(false),
        "the Background picker has no Alpha",
    );
    escape_picker(ctx, &mut app);
    // The slider moves the window.
    set_slider(ctx, &mut app, "term-acrylic", 50);
    let moved = ctx.until("50 to take the window to half", 4_000, &mut || {
        let g = ground_now(&mut app);
        g.ends_with("7f") || g.ends_with("80")
    });
    // PT: run.mjs:6404
    ctx.ok(
        moved,
        format!("50 takes the window to half ({})", ground_now(&mut app)),
    );
    set_slider(ctx, &mut app, "term-acrylic", 40);
    let back = ctx.until("40 to put the window back", 4_000, &mut || {
        let g = ground_now(&mut app);
        is_hex8(&g) && g.ends_with("99")
    });
    // PT: run.mjs:6406
    ctx.ok(back, "and 40 puts it back");

    // A theme pick with a level of your own asks first.
    let save = probe(&mut app, "picker.save", Value::Null);
    // PT: run.mjs:6409
    ctx.ok(
        save["dirty"] == json!(true),
        "a moved slider lights Save changes, as a changed Opacity did",
    );
    pick_card(ctx, &mut app, "nord");
    let asked = ask_open(ctx, &mut app, 3_000);
    // PT: run.mjs:6413
    ctx.ok(asked, "and a theme pick asks before it forgets it");
    answer_ask(ctx, &mut app, "cancel");
    let theme = pref(&app, "flare.term.theme");
    // PT: run.mjs:6416
    ctx.ok(
        ground_now(&mut app).ends_with("99") && theme.as_deref() == Some("dracula"),
        "Cancel keeps the theme and the see-through",
    );

    // 0 is off: the window solid.
    set_slider(ctx, &mut app, "term-acrylic", 0);
    let solid_now = ctx.until("the window to go solid", 4_000, &mut || {
        let g = ground_now(&mut app);
        is_hex8(&g) && g.ends_with("ff")
    });
    // PT: run.mjs:6420
    ctx.ok(
        solid_now,
        format!("at 0 the window is solid ({})", ground_now(&mut app)),
    );
    let said = str_of(&row(&mut app, "term-acrylic"), "num_text").to_string();
    // PT: run.mjs:6421
    ctx.ok(said == "Off", format!("and the slider says Off ({said})"));

    // 2. A Custom saved at 80, left live at 60.
    let custom = r##"{"bg":"#1d1f21","fg":"#c5c8c6","cursor":"#f0c674","ansi":{},"acrylic":true,"opacity":80}"##;
    let seed2 = [
        ("theme", "custom"),
        ("acrylic", "1"),
        ("opacity", "60"),
        ("custom", custom),
    ];
    let mut app = relaunch_with(ctx, &mut app, &seed2, None)?;
    let live = ctx.until("a Custom left live at 60 to paint 60", 10_000, &mut || {
        ground_now(&mut app) == "#1d1f2199"
    });
    // PT: run.mjs:6436
    ctx.ok(
        live,
        format!(
            "a Custom left live at 60 paints 60 ({})",
            ground_now(&mut app)
        ),
    );
    let slot = custom_json(&app);
    // PT: run.mjs:6438
    ctx.ok(
        slot["bg"] == json!("#1d1f21")
            && slot["seeThrough"] == json!(20)
            && slot.get("opacity").is_none()
            && slot.get("acrylic").is_none(),
        format!(
            "its saved 80 is its own level, 20, on an opaque ground ({})",
            slot
        ),
    );
    open_appearance(ctx, &mut app);
    pick_card(ctx, &mut app, "custom");
    if ask_open(ctx, &mut app, 2_000) {
        answer_ask(ctx, &mut app, "discard");
    }
    let restored = ctx.until("the saved 80 to come back", 4_000, &mut || {
        ground_now(&mut app) == "#1d1f21cc"
    });
    // PT: run.mjs:6447
    ctx.ok(
        restored,
        format!(
            "picking Custom again restores the saved 80 ({})",
            ground_now(&mut app)
        ),
    );
    // PT: run.mjs:6448
    let live_gone = settled_pref(ctx, &app, "flare.window.background").is_none();
    ctx.ok(live_gone, "and the live 60 went with the pick");
    ctx.shot(&mut app, "custom");
    // Leave solid: a pick of a preset puts its own 0 back.
    pick_card(ctx, &mut app, "dracula");
    if ask_open(ctx, &mut app, 1_500) {
        answer_ask(ctx, &mut app, "discard");
    }
    app.close();
    Ok(())
}

// ---- seeThrough ------------------------------------------------------------------------------------------

/// The alpha of the device pixel `(x, y)` (css px times the scale) of the last snapshot.
fn pixel_alpha(app: &mut App, x: f64, y: f64, k: f64) -> Option<u8> {
    match app.session.call(&Request::Pixel {
        x: (x * k).round().max(0.0) as u32,
        y: (y * k).round().max(0.0) as u32,
    }) {
        Ok(flare_e2e_proto::Reply::Pixel { rgba }) => Some(rgba[3]),
        _ => None,
    }
}

/// How much of the desktop each surface hides: the strip, the top row, the settings rail and the
/// page, from the Slint theme's tokens (1 is solid), and on the GPU renderer from the snapshot's alpha.
fn coats(app: &mut App, tmp: &Path) -> Vec<f64> {
    let paint = probe(app, "glass.paint", Value::Null);
    let a = &paint["alpha"];
    let mut out: Vec<f64> = ["tabs", "title", "side", "ground"]
        .iter()
        .map(|k| a[*k].as_f64().unwrap_or(-1.0))
        // A token with no alpha at all paints no coat of its own (the settings rail leaves the
        // ground's coat showing); the pixel read at the rail below proves the one coat there.
        .filter(|a| *a != 0.0)
        .collect();
    // The pixels: one device pixel inside each surface, alpha straight from the swapchain.
    if app.session.snapshot(&tmp.join("coats.png")).is_ok() {
        let k = scale_of(app);
        let grid = rect_of(&probe(app, "ui.rect", json!({ "name": "grid" })))
            .unwrap_or((0.0, 36.0, 1000.0, 600.0));
        let strip = rect_of(&probe(app, "ui.rect", json!({ "name": "strip" })));
        let spots = [
            strip.map(|s| (s.0 + s.2 - 6.0, s.1 + 12.0)),
            Some((3.0, 3.0)),
            Some((grid.0 + 20.0, grid.1 + grid.3 * 0.6)),
            Some((grid.0 + 260.0, grid.1 + 6.0)),
        ];
        for s in spots {
            let alpha = s.and_then(|(x, y)| pixel_alpha(app, x, y, k));
            out.push(alpha.map_or(-1.0, |a| (f64::from(a) / 255.0 * 100.0).round() / 100.0));
        }
    }
    out
}

fn all_near(v: &[f64], want: f64) -> bool {
    !v.is_empty() && v.iter().all(|a| (a - want).abs() < 0.015)
}

/// PT: run.mjs:6460-6579. The see-through slider: the row is named and placed as in Prism Terminal, right under
/// the wall, holds a slider (0 by default, solid), a level paints the window's ground at
/// 1 - level/100 on every surface as one coat, the Background picker carries no Alpha and a picked
/// colour leaves the level alone, a moved slider is a change to save, a theme pick asks and puts the
/// theme's own 0 back, and Custom keeps its level. The software renderer disables the slider and
/// paints solid (DIFF-5), so everything past the row's words is `skip`ped there.
pub fn see_through(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = launch_settings(ctx, &[], &[("FLARE_E2E_REDUCED_MOTION", "1")], None)?;
    let gpu = on_gpu(&mut app);
    const ID: &str = "term-acrylic";
    let tmp = ctx.ensure_world().root.clone();
    let opened = open_appearance(ctx, &mut app);
    let ids: Vec<String> = probe(&mut app, "settings.rows", json!({}))["rows"]
        .as_array()
        .map(|r| r.iter().map(|x| str_of(x, "id").to_string()).collect())
        .unwrap_or_default();
    let at = ids.iter().position(|i| i == "wall").unwrap_or(usize::MAX);
    // PT: run.mjs:6505
    ctx.ok(
        opened
            && ids.get(at.wrapping_add(1)).map(String::as_str) == Some(ID)
            && ids.get(at.wrapping_add(2)).map(String::as_str) == Some("window-background"),
        format!("the row sits right under the wall ({})", ids.join(" > ")),
    );
    reveal(ctx, &mut app, ID);
    let r = row(&mut app, ID);
    // DELIBERATE: PT's aria-label has a hyphen, but Flare's settings copy is plain words (letters, digits,
    // spaces, commas and full stops; copy_rules.rs), so the row reads "See through window".
    // PT: run.mjs:6506
    ctx.ok(
        str_of(&r, "kind") == "slider" && str_of(&r, "label") == "See through window",
        format!(
            "it is a slider called See through window ({} / {})",
            str_of(&r, "kind"),
            str_of(&r, "label")
        ),
    );
    // PT: run.mjs:6507. The software renderer says why the slider is off in place of its words.
    if gpu {
        ctx.ok(
            str_of(&r, "sub") == "How much of the desktop shows behind every surface.",
            format!("with its words ({})", str_of(&r, "sub")),
        );
    } else {
        ctx.skip(SOFTWARE_SKIP);
    }
    // PT: run.mjs:6508. A slider row has a slider and no switch: its kind is the one control.
    ctx.ok(str_of(&r, "kind") != "switch", "and no switch any more");
    // The tabs in the top row (the only layout since 2026-10-12): the strip is in the title row.
    let top = probe(&mut app, "ui.rect", json!({ "name": "strip" }));
    // PT: run.mjs:6513
    ctx.ok(
        rect_of(&top).is_some_and(|s| s.1 < 36.0),
        "the tabs are in the top row",
    );
    if !gpu || r["off"] == json!(true) {
        ctx.skip(SOFTWARE_SKIP);
        app.close();
        return Ok(());
    }

    // 0 by default: solid.
    pick_card(ctx, &mut app, "dracula");
    if ask_open(ctx, &mut app, 1_000) {
        answer_ask(ctx, &mut app, "discard");
    }
    reveal(ctx, &mut app, ID);
    let start = slider_num(&mut app, ID);
    // PT: run.mjs:6519
    ctx.ok(start == 0, format!("it starts at 0 ({start})"));
    // PT: run.mjs:6520
    ctx.ok(
        ground_now(&mut app).ends_with("ff"),
        format!("at 0 the window is solid ({})", ground_now(&mut app)),
    );
    // 30: the ground at 70%.
    set_slider(ctx, &mut app, ID, 30);
    let seventy = ctx.until("the ground to paint at 70%", 4_000, &mut || {
        (alpha_of_hex8(&ground_now(&mut app)) - 0.7).abs() < 0.01
    });
    // PT: run.mjs:6524
    ctx.ok(
        seventy,
        format!(
            "at 30 the ground is painted at 70% ({})",
            ground_now(&mut app)
        ),
    );
    // EVERY SURFACE IS ONE COAT (owner, 2026-10-11): a translucent box over another translucent box
    // is two coats, solid to the eye.
    let mut seen = Vec::new();
    ctx.until("every surface to show one coat at 0.7", 4_000, &mut || {
        seen = coats(&mut app, &tmp);
        all_near(&seen, 0.7)
    });
    // PT: run.mjs:6535
    ctx.ok(
        all_near(&seen, 0.7),
        format!("the strip, the top row, the settings rail and its page each show the desktop through one coat at 0.7 ({seen:?})"),
    );
    let said = str_of(&row(&mut app, ID), "num_text").to_string();
    // PT: run.mjs:6536
    ctx.ok(
        said == "30%",
        format!("the value is said beside it ({said})"),
    );
    // The keys move it. The slider needs the keys: a press on its track does not hand them over
    // (the touch area is above the focus scope), so this is the Right key to whatever has them.
    press(&mut app, "ArrowRight");
    let stepped = ctx.until("Right to step the slider", 2_000, &mut || {
        slider_num(&mut app, ID) == 31
    });
    // PT: run.mjs:6540
    ctx.ok(
        stepped,
        format!("Right steps it by one ({})", slider_num(&mut app, ID)),
    );
    press(&mut app, "ArrowLeft");
    // The top is 70, the 30% floor.
    let max = row(&mut app, ID)["num_max"].as_i64().unwrap_or(0);
    // PT: run.mjs:6544
    ctx.ok(max == 70, format!("it stops at 70 ({max})"));

    // The Background picker has no Alpha, and a picked colour leaves the level alone.
    open_swatch(ctx, &mut app, "window-background");
    // PT: run.mjs:6549
    ctx.ok(
        state(&mut app)["alpha_on"] == json!(false),
        "the Background picker has no Alpha",
    );
    escape_picker(ctx, &mut app);
    type_code(&mut app, "window-background", "#1c2330");
    let wears = ctx.until("a picked colour to wear the level", 4_000, &mut || {
        let g = ground_now(&mut app);
        g.starts_with("#1c2330") && (alpha_of_hex8(&g) - 0.7).abs() < 0.01
    });
    // FLARE-DEFECT: seeThrough / a picked colour wears the level / crates/flare/src/appctl.rs
    // (`on_key`: typing into a Settings code field is consumed with no shell in front).
    // PT: run.mjs:6556
    ctx.ok(
        wears,
        format!("a picked colour wears the level ({})", ground_now(&mut app)),
    );
    // The Reset link sits 10 px left of the code box.
    let code_box = row(&mut app, "window-background")["code_box"].clone();
    if let Some(b) = rect_of(&code_box) {
        click_at(
            &mut app,
            ((b.0 - 10.0 - 14.0) as f32, (b.1 + b.3 / 2.0) as f32),
        );
    }

    // A moved slider is a change to save; a theme pick asks, then puts its own 0 back.
    let save = probe(&mut app, "picker.save", Value::Null);
    // PT: run.mjs:6562
    ctx.ok(
        save["dirty"] == json!(true),
        "a moved slider lights Save changes",
    );
    pick_card(ctx, &mut app, "paper");
    let asked = ask_open(ctx, &mut app, 3_000);
    // PT: run.mjs:6565
    ctx.ok(asked, "and a theme pick asks first");
    answer_ask(ctx, &mut app, "discard");
    let own = ctx.until(
        "the new theme to come with its own 0, solid",
        4_000,
        &mut || slider_num(&mut app, ID) == 0 && ground_now(&mut app).ends_with("ff"),
    );
    // PT: run.mjs:6568
    ctx.ok(
        own,
        format!(
            "the new theme comes with its own 0, solid ({})",
            ground_now(&mut app)
        ),
    );
    let mut solid = Vec::new();
    ctx.until("every surface to be solid", 4_000, &mut || {
        solid = coats(&mut app, &tmp);
        all_near(&solid, 1.0)
    });
    // PT: run.mjs:6570
    ctx.ok(
        all_near(&solid, 1.0),
        format!("solid, every one of them is ({solid:?})"),
    );

    // Custom keeps its level: save at 45, leave, come back.
    set_slider(ctx, &mut app, ID, 45);
    // The slider sits far below the Save row: scroll it into the pane before pressing it.
    bring_into_view(ctx, &mut app, "the Save button", &mut |a| {
        let s = probe(a, "picker.save", Value::Null);
        Some((s["point"]["y"].as_f64()?, s["visible"] == json!(true)))
    });
    let sv = probe(&mut app, "picker.save", Value::Null);
    if let (Some(x), Some(y)) = (sv["point"]["x"].as_f64(), sv["point"]["y"].as_f64()) {
        click_at(&mut app, (x as f32, y as f32));
    }
    let lit_off = ctx.until("Save changes to keep it", 4_000, &mut || {
        probe(&mut app, "picker.save", Value::Null)["dirty"] == json!(false)
    });
    // PT: run.mjs:6577
    ctx.ok(lit_off, "Save changes keeps it");
    // The settings file is written a moment after the button goes quiet.
    ctx.poll(4_000, &mut || custom_json(&app)["seeThrough"] == json!(45));
    let kept = custom_json(&app)["seeThrough"].clone();
    // PT: run.mjs:6578
    ctx.ok(kept == json!(45), format!("as Custom's own level ({kept})"));
    pick_card(ctx, &mut app, "dracula");
    if ask_open(ctx, &mut app, 1_000) {
        answer_ask(ctx, &mut app, "discard");
    }
    reveal(ctx, &mut app, ID);
    // PT: run.mjs:6580
    ctx.ok(
        slider_num(&mut app, ID) == 0,
        "another theme is at its own 0",
    );
    pick_card(ctx, &mut app, "custom");
    if ask_open(ctx, &mut app, 1_000) {
        answer_ask(ctx, &mut app, "discard");
    }
    let back = ctx.until("Custom to bring its 45 back", 4_000, &mut || {
        slider_num(&mut app, ID) == 45
    });
    // PT: run.mjs:6583
    ctx.ok(
        back,
        format!(
            "and Custom brings its 45 back ({})",
            slider_num(&mut app, ID)
        ),
    );
    ctx.shot(&mut app, "see-through");
    // Leave on a quiet preset at 0 (a pick puts it back): nothing holds the close.
    pick_card(ctx, &mut app, "dracula");
    if ask_open(ctx, &mut app, 1_000) {
        answer_ask(ctx, &mut app, "discard");
    }
    app.close();
    Ok(())
}
