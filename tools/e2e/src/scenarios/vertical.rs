//! Vertical tabs scenarios (Task 23): verticalTabs.
//!
//! PT: run.mjs:2152-2819 (82 assertions, limit 360 s). Prism Terminal asks the page (`getBoundingClientRect`,
//! `getComputedStyle`, a mask position); Flare answers through the `sidebar.state` and `corner.state`
//! probes (e2e_probes/vertical.rs), the committed `app.state`, `ui.rect`, `settings.row` probes, and
//! painted pixels from a snapshot. A settings pill has no rectangle the probes read, so a segment is
//! picked by clicking along the row from its right end until the row's value is the wanted one.
//!
//! Where Flare differs: the `clip-path`, mask-position and transition checks are CSS, and the window
//! answers their native twins (the cut corner's pixels, the sidebar's edge at two heights while it
//! slides, the `sliding` flag). The four "main's cursor" checks need the real mouse, which a run never
//! moves; `flare-win::edge_watch` holds them in unit tests. The see-through checks need the GPU painter
//! (DIFF-5) and are skipped on software.

use std::path::Path;

use flare_e2e_proto::{Button, PointerAction, Request};
use serde_json::{json, Value};

use crate::ctx::{App, Ctx, Launch};
use crate::pixels::{bbox_where, Rgb};
use crate::world::{prompt_timeout, wait_for_prompt, Shell};

/// 225 percent (96 is 100 percent).
const DPI_225: u32 = 216;
/// 125 percent.
const DPI_125: u32 = 120;
/// The reason every glass check is skipped on the software renderer.
const SOFTWARE_SKIP: &str = "the software renderer paints a solid window (DIFF-5), so the see-through checks run on the GPU renderer";
/// The bar's height in css px.
const BAR_H: f64 = 36.0;

// ---------------------------------------------------------------- small helpers

/// A probe's answer, `Null` when it fails (a check on it then reads false).
fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// The sidebar probe's answer.
fn side(app: &mut App) -> Value {
    probe(app, "sidebar.state", Value::Null)
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// A css rectangle (`x`, `y`, `w`, `h`) as a tuple.
fn rect_of(v: &Value) -> Option<(f64, f64, f64, f64)> {
    Some((
        v["x"].as_f64()?,
        v["y"].as_f64()?,
        v["w"].as_f64()?,
        v["h"].as_f64()?,
    ))
}

fn centre(r: (f64, f64, f64, f64)) -> (f32, f32) {
    ((r.0 + r.2 / 2.0) as f32, (r.1 + r.3 / 2.0) as f32)
}

/// The `ui.rect` of a named surface, `None` when the window has none.
fn ui_rect(app: &mut App, name: &str) -> Option<(f64, f64, f64, f64)> {
    rect_of(&probe(app, "ui.rect", json!({ "name": name })))
}

/// The scale (1.0 is 100 percent).
fn scale(app: &mut App) -> f64 {
    // Read from the raw probe: `session.state()` cannot parse the vertical layout's tabs (they
    // have no strip rectangle), and the scale does not depend on it.
    probe(app, "app.state", Value::Null)["scale"]
        .as_f64()
        .unwrap_or(1.0)
}

fn send(app: &mut App, req: Request) -> bool {
    app.session.call(&req).is_ok()
}

fn move_to(app: &mut App, x: f64, y: f64) {
    send(
        app,
        Request::Pointer {
            x: x as f32,
            y: y as f32,
            action: PointerAction::Move,
        },
    );
}

fn drag(app: &mut App, from: (f64, f64), to: (f64, f64), steps: u32) -> bool {
    send(
        app,
        Request::Drag {
            from: (from.0 as f32, from.1 as f32),
            to: (to.0 as f32, to.1 as f32),
            steps,
            button: Button::Left,
        },
    )
}

fn key(app: &mut App, chord: &str) {
    let _ = app.session.key(chord);
}

fn click(app: &mut App, p: (f32, f32)) {
    let _ = app.session.click(p.0, p.1);
}

/// Holds the scenario still for `ms` in 50 ms steps (the most `settle` allows).
fn hold(ctx: &mut Ctx, ms: u64) {
    for _ in 0..(ms / 50) {
        ctx.settle(50);
    }
}

/// Every tab as `app.state` lists it.
fn tabs_of(app: &mut App) -> Vec<Value> {
    probe(app, "app.state", Value::Null)["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The kept rows of the sidebar, in order.
fn rows(s: &Value) -> Vec<Value> {
    s["rows"]
        .as_array()
        .map(|r| {
            r.iter()
                .filter(|x| x["kept"] == json!(true))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn labels(s: &Value) -> Vec<String> {
    rows(s)
        .iter()
        .map(|r| r["label"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The centre of row `i`'s card, in css px.
fn row_centre(s: &Value, i: usize) -> Option<(f32, f32)> {
    rows(s).get(i).and_then(|r| rect_of(&r["rect"])).map(centre)
}

/// Types a line into the shell in front once it shows its prompt.
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

/// PT's `hookSay` for the flare-agent protocol.
fn say(state: &str, extra: &str) -> String {
    format!("[Console]::Write([char]27 + ']777;flare-agent;state={state}{extra}' + [char]7)")
}

/// The stored settings (`<profile>\settings.json`), or an empty object.
fn store(app: &App) -> Value {
    let text = std::fs::read_to_string(app.profile.join("settings.json")).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_else(|_| json!({}))
}

fn stored(app: &App, key: &str) -> String {
    store(app)[key].as_str().unwrap_or("").to_string()
}

// ---------------------------------------------------------------- pixels

/// A decoded snapshot: device pixels, row-major.
struct Snap {
    w: i64,
    h: i64,
    px: Vec<Rgb>,
}

impl Snap {
    /// The colour at a device pixel, clamped to the snapshot.
    fn at(&self, x: i64, y: i64) -> Rgb {
        let (x, y) = (x.clamp(0, self.w - 1), y.clamp(0, self.h - 1));
        self.px[(y * self.w + x) as usize]
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
        w: i64::from(info.width),
        h: i64::from(info.height),
        px,
    })
}

/// A snapshot of the window, decoded; `None` when it failed.
fn snap(ctx: &mut Ctx, app: &mut App, name: &str) -> Option<Snap> {
    let path = ctx.shot(app, name)?;
    read_png(&path)
}

fn rgb_of_hex(hex: &str) -> Rgb {
    let h = hex.trim_start_matches('#');
    let ch = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    [ch(0), ch(2), ch(4)]
}

/// The alpha of a `#rrggbbaa` token, 255 for `#rrggbb`.
fn alpha_of_hex(hex: &str) -> u8 {
    let h = hex.trim_start_matches('#');
    if h.len() >= 8 {
        u8::from_str_radix(&h[6..8], 16).unwrap_or(255)
    } else {
        255
    }
}

fn dist(a: Rgb, b: Rgb) -> i32 {
    (0..3)
        .map(|i| (i32::from(a[i]) - i32::from(b[i])).abs())
        .sum()
}

/// True when two colours are within `tol` (sum of channel differences).
fn near(a: Rgb, b: Rgb, tol: i32) -> bool {
    dist(a, b) <= tol
}

/// True when two colours differ enough to be told apart (a hairline is about 12).
fn differs(a: Rgb, b: Rgb) -> bool {
    dist(a, b) >= 6
}

/// A css length in device pixels.
fn dv(css: f64, k: f64) -> i64 {
    (css * k).round() as i64
}

/// The theme colour the sidebar probe names, as rgb.
fn token(s: &Value, name: &str) -> Rgb {
    rgb_of_hex(s["theme"][name].as_str().unwrap_or("#000000"))
}

/// The first device column at row `y` (from the left edge) whose colour is not `ground`.
fn run_end(s: &Snap, y: i64, ground: Rgb) -> i64 {
    let mut x = 0;
    while x < s.w && near(s.at(x, y), ground, 8) {
        x += 1;
    }
    x
}

/// How far the cut corner bulges past the sidebar's straight edge, in device px: the secondary run
/// just under the bar minus the same run in the empty part of the panel. `None` when the panel is
/// off the window at that moment.
fn bulge(s: &Snap, k: f64, secondary: Rgb) -> Option<i64> {
    let low = run_end(s, dv(500.0, k), secondary);
    if low < dv(40.0, k) {
        return None;
    }
    Some(run_end(s, dv(BAR_H, k) + 1, secondary) - low)
}

/// Checks the corner where the bar meets the sidebar at the sidebar's right edge `right` (css px):
/// outside the arc is the bar's colour, inside it is the terminal's, and there is no seam with the
/// bar above or the sidebar left of it. `Err` says what is wrong.
fn corner_cut(s: &Snap, k: f64, right: f64, secondary: Rgb) -> Result<(), String> {
    let (x0, y0) = (dv(right, k), dv(BAR_H, k));
    let out = s.at(x0 + 1, y0 + 1);
    if !near(out, secondary, 8) {
        return Err(format!(
            "outside the arc is {out:?}, not the sidebar's {secondary:?}"
        ));
    }
    // Inside the arc is the terminal, whose first line of text may run through the point: it is
    // anything but the sidebar's own colour.
    let inn = s.at(x0 + dv(6.0, k), y0 + dv(6.0, k));
    if near(inn, secondary, 3) && !near(inn, s.at(x0 + dv(60.0, k), y0 + dv(60.0, k)), 3) {
        return Err(format!(
            "inside the arc is {inn:?}, still the sidebar's colour"
        ));
    }
    let above = s.at(x0 - dv(20.0, k), y0 - 2);
    if !near(above, secondary, 8) {
        return Err(format!("the bar above the sidebar is {above:?}"));
    }
    let left = s.at(x0 - 3, y0 + dv(200.0, k));
    if !near(left, secondary, 8) {
        return Err(format!("the sidebar's own edge is {left:?}"));
    }
    Ok(())
}

/// The ink box (css px) of the pixels in a css region that differ from `ground` by more than `by`.
fn ink_box(
    s: &Snap,
    k: f64,
    region: (f64, f64, f64, f64),
    ground: Rgb,
    by: i32,
) -> Option<(f64, f64, f64, f64)> {
    let (x, y, w, h) = region;
    let (ax, ay) = (dv(x, k).max(0), dv(y, k).max(0));
    let (rw, rh) = ((dv(w, k)).max(1) as u32, (dv(h, k)).max(1) as u32);
    let found = bbox_where(
        rw,
        rh,
        |px, py| s.at(ax + i64::from(px), ay + i64::from(py)),
        |c| dist(c, ground) > by,
    )?;
    Some((
        (ax + i64::from(found.0)) as f64 / k,
        (ay + i64::from(found.1)) as f64 / k,
        f64::from(found.2) / k,
        f64::from(found.3) / k,
    ))
}

/// Every pixel of a css region is near `ground`: nothing of another surface shows through it.
fn region_clean(s: &Snap, k: f64, region: (f64, f64, f64, f64), ground: Rgb) -> bool {
    ink_box(s, k, region, ground, 8).is_none()
}

// ---------------------------------------------------------------- settings

/// True while the Settings tab is in front.
fn settings_front(app: &mut App) -> bool {
    probe(app, "settings.state", Value::Null)["front"] == json!(true)
}

fn settings_open(app: &mut App) -> bool {
    probe(app, "settings.state", Value::Null)["open"] == json!(true)
}

/// One row of the open page (`settings.row`), `Null` when the page has no such row.
fn row(app: &mut App, id: &str) -> Value {
    probe(app, "settings.row", json!({ "id": id }))
}

/// The catalogue (`settings.rows` with `catalogue`).
fn catalogue(app: &mut App) -> Vec<Value> {
    probe(app, "settings.rows", json!({ "catalogue": true }))["rows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn page_of(app: &mut App, id: &str) -> Option<String> {
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
fn open_settings(ctx: &mut Ctx, app: &mut App) -> bool {
    if settings_front(app) {
        return true;
    }
    match ui_rect(app, "cog") {
        Some(r) => click(app, centre(r)),
        None => return false,
    }
    ctx.until("Settings is in front", 8_000, &mut || settings_front(app))
}

/// Chooses a rail page by its rectangle and waits until its rows answer for it.
fn goto_page(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    if let Some(r) = rect_of(&probe(app, "settings.rail", json!({ "id": id }))) {
        click(app, centre(r));
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

/// PT's `gotoPref`: Settings open, the page that holds the row shown, the row in view.
fn goto_row(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    let Some(page) = page_of(app, id) else {
        return false;
    };
    if !open_settings(ctx, app) {
        return false;
    }
    if page_now(app).as_deref() != Some(page.as_str()) && !goto_page(ctx, app, &page) {
        return false;
    }
    reveal(ctx, app, id)
}

/// True when the row's value is one of `want` (compared in lower case).
fn row_is(app: &mut App, id: &str, want: &[&str]) -> bool {
    let v = row(app, id)["value"].as_str().unwrap_or("").to_lowercase();
    want.iter().any(|w| v == *w)
}

/// PT's `setPref`: picks a segment of a pill row by clicking along the row from its right end until
/// the row says the wanted value. A pill is right-aligned, 16 px in from the row's edge.
fn pick_seg(ctx: &mut Ctx, app: &mut App, id: &str, want: &[&str]) -> bool {
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
        click(app, (cx as f32, (y + h / 2.0) as f32));
        for _ in 0..4 {
            if row_is(app, id, want) {
                return true;
            }
            ctx.settle(25);
        }
    }
    row_is(app, id, want)
}

/// Wheels the pane until `find` says its subject is in view (the same loop `reveal` runs).
fn bring_into_view(
    ctx: &mut Ctx,
    app: &mut App,
    what: &str,
    find: &mut dyn FnMut(&mut App) -> Option<(f64, bool)>,
) -> bool {
    let (gx, gy, gw, gh) = ui_rect(app, "grid").unwrap_or((0.0, 36.0, 800.0, 600.0));
    let at = ((gx + gw / 2.0) as f32, (gy + gh / 2.0) as f32);
    ctx.until(
        &format!("{what} to be in view"),
        12_000,
        &mut || match find(app) {
            Some((_, true)) => true,
            Some((y, false)) => {
                let dy = if y > gy + gh / 2.0 { -160.0 } else { 160.0 };
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

/// Picks a theme card by id: the wall is opened so every card is a target, the card is wheeled into
/// view and pressed (`picker.wall` answers for the wall and its cards).
fn pick_card_in_view(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    let card_of = |app: &mut App| -> Option<Value> {
        probe(app, "picker.wall", Value::Null)["cards"]
            .as_array()?
            .iter()
            .find(|c| c["id"] == json!(id))
            .cloned()
    };
    if probe(app, "picker.wall", Value::Null)["expanded"] == json!(false) {
        bring_into_view(ctx, app, "the wall's toggle", &mut |a| {
            let w = probe(a, "picker.wall", Value::Null);
            Some((
                w["toggle"]["y"].as_f64()?,
                w["toggle_visible"] == json!(true),
            ))
        });
        if let Some(r) = rect_of(&probe(app, "picker.wall", Value::Null)["toggle"]) {
            click(app, centre(r));
        }
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
    match card_of(app).and_then(|c| rect_of(&c["rect"])) {
        Some(r) => {
            click(app, centre(r));
            true
        }
        None => false,
    }
}
/// PT's `backToFirst`: Settings closed, the first tab in front.
fn back_to_first(ctx: &mut Ctx, app: &mut App) {
    if settings_open(app) {
        if let Some(r) = ui_rect(app, "cog") {
            if settings_front(app) {
                click(app, centre(r));
            } else if let Some(p) = settings_tab_centre(app) {
                // Not in front: the cog would bring it forward, so close it from its own row.
                click(app, p);
                if let Some(r) = ui_rect(app, "cog") {
                    click(app, centre(r));
                }
            }
        }
        ctx.until("Settings is closed", 4_000, &mut || !settings_open(app));
    }
    let s = side(app);
    if s["vertical"] == json!(true) {
        if let Some(p) = row_centre(&s, 0) {
            if s["shown"] == json!(true) {
                click(app, p);
            }
        }
    } else if let Some(t) = tabs_of(app).first() {
        if let Some(r) = rect_of(&t["rect"]) {
            click(app, centre(r));
        }
    }
}

/// The centre of the Settings row in the sidebar or the strip, when it exists.
fn settings_tab_centre(app: &mut App) -> Option<(f32, f32)> {
    let s = side(app);
    if s["vertical"] == json!(true) {
        let list = rows(&s);
        let i = list.iter().position(|r| r["settings"] == json!(true))?;
        return row_centre(&s, i);
    }
    tabs_of(app)
        .iter()
        .find(|t| t["kind"] == json!("settings"))
        .and_then(|t| rect_of(&t["rect"]))
        .map(centre)
}

// ---------------------------------------------------------------- the scenario

/// PT `verticalTabs` (run.mjs:2152-2819).
pub fn vertical_tabs(ctx: &mut Ctx) -> Result<(), String> {
    let root = ctx.ensure_world().root.clone();
    let folders: Vec<std::path::PathBuf> =
        ["FlareTerminal", "notes", "api", "web", "infra", "docs"]
            .iter()
            .map(|n| {
                let d = root.join(n);
                let _ = std::fs::create_dir_all(&d);
                d
            })
            .collect();
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders,
        scale_dpi: Some(96),
        ..Launch::default()
    })?;
    let gpu = app
        .hello()
        .map(|h| h.renderer.to_lowercase().starts_with("gpu"))
        .unwrap_or(false);
    send(
        &mut app,
        Request::Resize {
            w: 1100.0,
            h: 700.0,
        },
    );
    mark_strip(ctx, &mut app);
    vertical_a(ctx, &mut app)?;
    vertical_b(ctx, &mut app, gpu)?;
    app.close();
    Ok(())
}

/// Snapshots taken while the collapsed sidebar slides to `to_shown`: PT looked at every animation
/// frame, and a snapshot is slower than a frame, so the slide is started again (the other way, then
/// this way) until one snapshot has caught the panel between its two ends, four times at most.
fn slide_samples(
    ctx: &mut Ctx,
    app: &mut App,
    name: &str,
    to_shown: bool,
    secondary: Rgb,
    k: f64,
) -> Vec<Snap> {
    let w = num(&side(app)["nav"]["w"]);
    let between = |sn: &Snap| {
        let low = run_end(sn, dv(500.0, k), secondary);
        low >= dv(40.0, k) && low < dv(w, k) - dv(8.0, k)
    };
    let go = |app: &mut App, shown: bool| {
        if shown {
            move_to(app, 2.0, 400.0);
        } else {
            move_to(app, 800.0, 400.0);
        }
    };
    let mut out = Vec::new();
    for attempt in 0..4 {
        go(app, to_shown);
        let mut caught = false;
        for i in 0..10 {
            if let Some(sn) = snap(ctx, app, &format!("{name}-{i}")) {
                caught |= between(&sn);
                out.push(sn);
            }
        }
        if caught || attempt == 3 {
            break;
        }
        // Not caught: back to where it started, then again.
        go(app, !to_shown);
        ctx.until("the sidebar to reach its start", 3_000, &mut || {
            side(app)["shown"] == json!(!to_shown)
        });
        hold(ctx, 500);
    }
    out
}

/// PT `markStrip` (run.mjs:349): six tabs put into the mockups' states, left to right.
fn mark_strip(ctx: &mut Ctx, app: &mut App) {
    let states = |app: &mut App| -> Vec<String> {
        tabs_of(app)
            .iter()
            .map(|t| t["mark"]["state"].as_str().unwrap_or("").to_string())
            .collect()
    };
    let six = ctx.until("six tabs", 30_000, &mut || tabs_of(app).len() == 6);
    if six {
        for i in [0usize, 2, 3, 4, 5] {
            if let Some(r) = rect_of(&tabs_of(app)[i]["rect"]) {
                click(app, centre(r));
            }
            type_line(app, &say("working", ""));
        }
        // A hook line typed the moment a shell showed its prompt can be lost on a busy machine: any
        // tab that is not working yet is told again (twice at most), so the set-up is not a race.
        for _ in 0..2 {
            hold(ctx, 700);
            let got = states(app);
            for i in [0usize, 2, 3, 4, 5] {
                if got.get(i).map(String::as_str) != Some("working") {
                    if let Some(r) = rect_of(&tabs_of(app)[i]["rect"]) {
                        click(app, centre(r));
                    }
                    type_line(app, &say("working", ""));
                }
            }
        }
        let extra = [
            (
                3usize,
                format!("Start-Sleep -Milliseconds 9000; {}", say("done", "")),
            ),
            (
                4,
                format!("Start-Sleep -Milliseconds 8000; {}", say("question", "")),
            ),
            (
                5,
                format!(
                    "Start-Sleep -Milliseconds 7000; {}; {}",
                    say("failed", ";kind=rate_limit"),
                    say("failed", "")
                ),
            ),
        ];
        for (i, line) in extra {
            if let Some(r) = rect_of(&tabs_of(app)[i]["rect"]) {
                click(app, centre(r));
            }
            type_line(app, &line);
        }
        if let Some(r) = rect_of(&tabs_of(app)[0]["rect"]) {
            click(app, centre(r));
        }
    }
    let want: Vec<String> = [
        "working", "none", "working", "finished", "question", "failed",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let settled = ctx.until("the six tabs to take their states", 25_000, &mut || {
        states(app) == want
    });
    let got = states(app);
    // PT: run.mjs:349 (markStrip's one check).
    ctx.ok(
        settled,
        format!("six tabs in the mockups' states ({got:?})"),
    );
}

/// Checks 1 to 41 of PT's scenario: the setting, the geometry, the edge, the slide, the grip, the
/// find box, the marks and the drag.
fn vertical_a(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    // ---- the setting (PT 2162-2171)
    let on_row = goto_row(ctx, app, "tab-layout");
    let r = row(app, "tab-layout");
    // PT: run.mjs:2163
    ctx.ok(
        on_row && row_is(app, "tab-layout", &["horizontal"]),
        format!(
            "Horizontal is the default ({})",
            r["value"].as_str().unwrap_or("")
        ),
    );
    let names: Vec<String> = r["options"]
        .as_array()
        .map(|o| {
            o.iter()
                .map(|n| n.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    // PT: run.mjs:2164
    ctx.ok(
        names.join("|") == "Horizontal|Vertical",
        format!("the choices are Horizontal and Vertical ({names:?})"),
    );
    // PT: run.mjs:2166. The Window section's first row: the first catalogue row of tab-layout's
    // section is tab-layout itself.
    let cat = catalogue(app);
    let section = cat
        .iter()
        .find(|c| c["id"] == json!("tab-layout"))
        .map(|c| c["section"].clone());
    let first = cat
        .iter()
        .find(|c| Some(&c["section"]) == section.as_ref())
        .map(|c| c["id"].as_str().unwrap_or("").to_string())
        .unwrap_or_default();
    ctx.ok(
        first == "tab-layout",
        format!("Tab position is the Window section's first row ({first})"),
    );
    let picked = pick_seg(ctx, app, "tab-layout", &["vertical"]);
    let wrote = picked
        && ctx.until("Vertical to be stored", 5_000, &mut || {
            stored(app, "flare.window.tabLayout") == "vertical"
        });
    // PT: run.mjs:2169
    ctx.ok(wrote, "Vertical is stored");
    back_to_first(ctx, app);

    // ---- the geometry at 100 percent (PT 2201-2231)
    ctx.until("the sidebar", 6_000, &mut || {
        side(app)["open"] == json!(true)
    });
    ctx.settle(50);
    let s = side(app);
    let k = scale(app);
    let list = rows(&s);
    let shot = snap(ctx, app, "open");
    let secondary = token(&s, "secondary");
    let ground = token(&s, "ground");
    let nav_w = num(&s["nav"]["w"]);
    // PT: run.mjs:2201
    ctx.ok(
        s["strip"] == json!(false) && s["open"] == json!(true),
        format!("the strip is gone and the sidebar is there ({})", s["nav"]),
    );
    let tops: Vec<f64> = list.iter().map(|r| num(&r["rect"]["y"])).collect();
    // PT: run.mjs:2202
    ctx.ok(
        tops.len() == 6 && tops.windows(2).all(|w| w[1] > w[0]),
        format!("six rows, one under another ({tops:?})"),
    );
    let lefts: Vec<i64> = list
        .iter()
        .map(|r| num(&r["rect"]["x"]).round() as i64)
        .collect();
    // PT: run.mjs:2203
    ctx.ok(
        !lefts.is_empty() && lefts.iter().all(|l| *l == lefts[0]),
        "all in one column",
    );
    let (grid_x, _, _, _) = ui_rect(app, "grid").unwrap_or((0.0, 0.0, 0.0, 0.0));
    // PT: run.mjs:2204
    ctx.ok(
        num(&s["nav"]["x"]) == 0.0
            && nav_w >= 150.0
            && num(&s["host_left"]) >= num(&s["nav"]["right"]) - 1.0,
        format!(
            "the sidebar on the left, the terminal beside it ({nav_w}px, terminal at {grid_x})"
        ),
    );
    // PT: run.mjs:2205. The band is a square the card's height at its left, and the icon in it is
    // 18 px: the ink inside the band is measured from the snapshot.
    let front = list.iter().find(|r| r["front"] == json!(true));
    let band_ok = front.is_some_and(|f| {
        let b = &f["icon_band"];
        let card = &f["rect"];
        let square = num(&b["w"]) == num(&b["h"])
            && num(&b["h"]) == num(&card["h"])
            && (num(&b["x"]) - num(&card["x"])).abs() < 0.01;
        let ink = shot.as_ref().and_then(|sn| {
            let (bx, by, bw, bh) = (num(&b["x"]), num(&b["y"]), num(&b["w"]), num(&b["h"]));
            let bg = sn.at(dv(bx + 2.0, k), dv(by + 2.0, k));
            ink_box(sn, k, (bx + 3.0, by + 3.0, bw - 6.0, bh - 6.0), bg, 60)
        });
        square && ink.is_some_and(|i| (11.0..=19.0).contains(&i.2))
    });
    ctx.ok(
        band_ok,
        format!(
            "each card has its square icon band at the left, an 18 px icon in it ({})",
            front
                .map(|f| f["icon_band"].to_string())
                .unwrap_or_default()
        ),
    );
    // PT: run.mjs:2206. The card in front is tinted: its fill differs from a card behind.
    let tint = shot.as_ref().is_some_and(|sn| {
        let pick = |front: bool| {
            list.iter()
                .find(|r| (r["front"] == json!(true)) == front)
                .map(|r| {
                    sn.at(
                        dv(num(&r["rect"]["x"]) + 120.0, k),
                        dv(num(&r["rect"]["y"]) + 3.0, k),
                    )
                })
        };
        match (pick(true), pick(false)) {
            (Some(a), Some(b)) => differs(a, b),
            _ => false,
        }
    });
    ctx.ok(tint, "the card in front is tinted");
    // PT: run.mjs:2227. The CSS clip-path has no native meaning; its twin is "no seam": the bar's
    // colour right above the sidebar is the sidebar's own.
    let seam = shot.as_ref().is_some_and(|sn| {
        let x = dv(nav_w / 2.0 + 20.0, k);
        near(sn.at(x, dv(BAR_H, k) - 3), secondary, 8)
            && near(sn.at(x, dv(BAR_H, k) + 3), secondary, 8)
    });
    ctx.ok(
        seam,
        "no seam where the bar meets the sidebar (the CSS clip-path has no native meaning)",
    );
    // PT: run.mjs:2229. The terminal is cut round where the bar and the sidebar meet and the bar
    // draws no line; sampled at 100 percent, then at 225 percent.
    let corner_100 = shot
        .as_ref()
        .map(|sn| corner_cut(sn, k, nav_w, secondary))
        .unwrap_or_else(|| Err("no snapshot".into()));
    let mut corner_225: Result<(), String> = Err("the scale did not change".into());
    if send(
        app,
        Request::DpiChange {
            dpi: DPI_225,
            x: None,
            y: None,
        },
    ) && ctx.until("225 percent", 6_000, &mut || scale(app) > 2.2)
    {
        hold(ctx, 300);
        let k2 = scale(app);
        let s2 = side(app);
        let w2 = num(&s2["nav"]["w"]);
        corner_225 = match snap(ctx, app, "open-225") {
            Some(sn) => corner_cut(&sn, k2, w2, secondary),
            None => Err("no snapshot".into()),
        };
    }
    send(
        app,
        Request::DpiChange {
            dpi: 96,
            x: None,
            y: None,
        },
    );
    ctx.until("100 percent", 6_000, &mut || scale(app) < 1.1);
    hold(ctx, 200);
    ctx.ok(
        corner_100.is_ok() && corner_225.is_ok(),
        format!("the terminal is cut round where the bar and the sidebar meet, and the bar draws no line (100%: {corner_100:?}, 225%: {corner_225:?})"),
    );
    let k = scale(app);

    // ---- the edge (PT 2236-2276)
    let line_on = |sn: &Snap, nav_w: f64, secondary: Rgb| -> bool {
        let (x0, y0) = (dv(nav_w, k), dv(BAR_H, k));
        let top = differs(
            sn.at(x0 + dv(100.0, k), y0),
            sn.at(x0 + dv(100.0, k), y0 + dv(3.0, k)),
        );
        let left = differs(
            sn.at(x0, y0 + dv(100.0, k)),
            sn.at(x0 + dv(4.0, k), y0 + dv(100.0, k)),
        );
        // The open sidebar draws nothing of its own: its last column is its own colour.
        let clean = near(sn.at(x0 - 1, y0 + dv(300.0, k)), secondary, 8);
        top && left && clean
    };
    // PT: run.mjs:2252
    ctx.ok(
        shot.as_ref()
            .is_some_and(|sn| line_on(sn, nav_w, secondary)),
        "PT Default: one line along the bar and down the terminal's edge; the sidebar draws none",
    );
    // PT: run.mjs:2263
    ctx.ok(
        differs(secondary, ground),
        format!(
            "and the sidebar a step darker than the terminal ({secondary:?} against {ground:?})"
        ),
    );
    let mut campbell = false;
    if goto_row(ctx, app, "term-theme") && pick_card_in_view(ctx, app, "campbell") {
        ctx.until("Campbell's colours", 6_000, &mut || {
            token(&side(app), "ground") != ground
        });
        back_to_first(ctx, app);
        hold(ctx, 300);
        let s3 = side(app);
        if let Some(sn) = snap(ctx, app, "campbell") {
            campbell = line_on(&sn, num(&s3["nav"]["w"]), token(&s3, "secondary"));
        }
    }
    // PT: run.mjs:2266
    ctx.ok(campbell, "Campbell, black on black: the same line");
    // PT: run.mjs:2282. Panel edges: None hides the line, Solid makes it stronger.
    let alpha = |app: &mut App| -> u8 {
        alpha_of_hex(side(app)["theme"]["divider"].as_str().unwrap_or("#000000"))
    };
    let hair = alpha(app);
    let mut none = 255;
    let mut solid = 0;
    if pick_seg(ctx, app, "window-edges", &["none"]) {
        hold(ctx, 150);
        none = alpha(app);
    }
    if pick_seg(ctx, app, "window-edges", &["solid"]) {
        hold(ctx, 150);
        solid = alpha(app);
    }
    pick_seg(ctx, app, "window-edges", &["hairline"]);
    ctx.ok(
        none == 0 && hair > 0 && solid > hair,
        format!("the line follows Panel edges: None {none}, Hairline {hair}, Solid {solid}"),
    );
    if goto_row(ctx, app, "term-theme") {
        pick_card_in_view(ctx, app, "pt-default");
        hold(ctx, 300);
    }
    back_to_first(ctx, app);

    // ---- slid out (PT 2286-2350)
    key(app, "Ctrl+Shift+B");
    move_to(app, 800.0, 400.0);
    ctx.until("the sidebar to be out of sight", 4_000, &mut || {
        let s = side(app);
        s["collapsed"] == json!(true) && s["shown"] == json!(false)
    });
    hold(ctx, 500);
    let secondary = token(&side(app), "secondary");
    // The settled slid-out corner is the reference every moving sample is held to.
    let samples = slide_samples(ctx, app, "opening", true, secondary, k);
    ctx.until("the sidebar to slide out", 3_000, &mut || {
        side(app)["shown"] == json!(true)
    });
    hold(ctx, 400);
    let shown = side(app);
    let nav_w = num(&shown["nav"]["w"]);
    let settled = snap(ctx, app, "slid");
    let reference = settled.as_ref().and_then(|sn| bulge(sn, k, secondary));
    let worst = |samples: &[Snap]| -> f64 {
        let Some(rf) = reference else { return f64::MAX };
        samples
            .iter()
            .filter_map(|sn| bulge(sn, k, secondary))
            .map(|b| (b - rf).abs() as f64 / k)
            .fold(0.0, f64::max)
    };
    let opening = worst(&samples);
    // PT: run.mjs:2299
    ctx.ok(
        reference.is_some() && opening <= 1.0,
        format!("sliding out, the corner keeps to the sidebar's edge, no band beside the line (worst {opening:.1}px)"),
    );
    let samples = slide_samples(ctx, app, "closing", false, secondary, k);
    let closing = worst(&samples);
    // PT: run.mjs:2303
    ctx.ok(
        reference.is_some() && closing <= 1.0,
        format!("and sliding away (worst {closing:.1}px)"),
    );
    move_to(app, 2.0, 400.0);
    ctx.until("the sidebar to show", 3_000, &mut || {
        side(app)["shown"] == json!(true)
    });
    hold(ctx, 300);
    // PT: run.mjs:2318. Slid out, a line from the bar down (not up into the bar), the page's own line
    // along the top, and no shadow beside it.
    let slid = snap(ctx, app, "slid-line");
    let (x0, y0) = (dv(nav_w, k), dv(BAR_H, k));
    let slid_line = slid.as_ref().is_some_and(|sn| {
        let down = differs(
            sn.at(x0 - 1, y0 + dv(300.0, k)),
            sn.at(x0 - dv(8.0, k), y0 + dv(300.0, k)),
        );
        let not_up = near(sn.at(x0 - 1, y0 - 6), secondary, 8);
        let top = differs(
            sn.at(x0 + dv(100.0, k), y0),
            sn.at(x0 + dv(100.0, k), y0 + dv(3.0, k)),
        );
        let inside = sn.at(x0 + dv(60.0, k), y0 + dv(60.0, k));
        let no_shadow = near(sn.at(x0 + dv(5.0, k), y0 + dv(300.0, k)), inside, 12);
        down && not_up && top && no_shadow
    });
    ctx.ok(
        slid_line,
        "slid out, a line from the bar down, the bar's own line along the top, no shadow",
    );
    // PT: run.mjs:2319
    let corner_slid = slid
        .as_ref()
        .map(|sn| corner_cut(sn, k, nav_w, secondary))
        .unwrap_or_else(|| Err("no snapshot".into()));
    let cs = probe(app, "corner.state", Value::Null);
    ctx.ok(
        corner_slid.is_ok() && cs["slid"] == json!(true),
        format!("and the corner, filled to the curve, carries the line round into the bar ({corner_slid:?})"),
    );
    // PT: run.mjs:2325. SLID OUT, IT RESIZES TOO.
    let g = side(app)["grip"].clone();
    let g_pos = rect_of(&g).map(|(x, y, w, _)| (x + w / 2.0, y + 300.0));
    // PT: run.mjs:2331
    ctx.ok(g_pos.is_some(), "slid out, the sidebar has its resize grip");
    if let Some((gx, gy)) = g_pos {
        let before = num(&side(app)["nav"]["w"]);
        drag(app, (gx, gy), (gx + 40.0, gy), 8);
        hold(ctx, 300);
        let after = side(app);
        let aw = num(&after["nav"]["w"]);
        // PT: run.mjs:2349
        ctx.ok(
            aw >= before + 35.0 && after["shown"] == json!(true),
            format!("dragging it widens the slid-out sidebar, which stays out ({before} to {aw})"),
        );
        // The pointer rests on the sidebar, off the grip, whose hover line would cross the corner.
        move_to(app, aw / 2.0, 450.0);
        hold(ctx, 150);
        let moved = snap(ctx, app, "slid-wide");
        let b = moved.as_ref().and_then(|sn| bulge(sn, k, secondary));
        let keeps = match (b, reference) {
            (Some(b), Some(r)) => (b - r).abs() as f64 / k <= 1.0,
            _ => false,
        };
        // PT: run.mjs:2350
        ctx.ok(
            keeps,
            format!("and the corner keeps to its new edge ({b:?} against {reference:?})"),
        );
        // Back to the width it had.
        let (nx, ny) = (aw - 3.0, gy);
        drag(app, (nx + 3.0, ny), (nx + 3.0 - 40.0, ny), 8);
        hold(ctx, 300);
    } else {
        ctx.ok(
            false,
            "dragging it widens the slid-out sidebar, which stays out (no grip)",
        );
        ctx.ok(false, "and the corner keeps to its new edge (no grip)");
    }
    move_to(app, 800.0, 400.0);
    hold(ctx, 500);
    key(app, "Ctrl+Shift+B");
    ctx.until("the sidebar pinned", 4_000, &mut || {
        side(app)["open"] == json!(true)
    });
    hold(ctx, 400);

    // ---- narrow Settings beside the sidebar (PT 2354-2373)
    send(app, Request::Resize { w: 900.0, h: 700.0 });
    hold(ctx, 300);
    let opened = goto_row(ctx, app, "tab-layout");
    hold(ctx, 400);
    let s = side(app);
    let nav_w = num(&s["nav"]["w"]);
    let k = scale(app);
    let (frame_left, _, _, _) = ui_rect(app, "grid").unwrap_or((0.0, 0.0, 0.0, 0.0));
    // The rows end under the last card; the empty panel below them must show nothing of the page.
    let rows_end = BAR_H + 50.0 + (rows(&s).len() as f64) * 50.0 + 12.0;
    let leak = snap(ctx, app, "settings-narrow").is_some_and(|sn| {
        let foot = num(&s["window"]["h"]) - 60.0;
        region_clean(
            &sn,
            k,
            (2.0, rows_end, nav_w - 12.0, foot - rows_end),
            secondary,
        )
    });
    // PT: run.mjs:2371
    ctx.ok(
        opened && leak && frame_left >= num(&s["nav"]["right"]) - 1.0,
        format!("narrow Settings beside the sidebar keeps its search inside the page (frame at {frame_left}, sidebar to {})", s["nav"]["right"]),
    );
    send(
        app,
        Request::Resize {
            w: 1100.0,
            h: 700.0,
        },
    );
    hold(ctx, 300);
    back_to_first(ctx, app);

    // ---- the folder line, the find box (PT 2375-2412)
    let s = side(app);
    let folders: Vec<String> = rows(&s)
        .iter()
        .map(|r| r["folder"].as_str().unwrap_or("").to_string())
        .collect();
    // PT: run.mjs:2377
    ctx.ok(
        folders.len() == 6 && folders.iter().all(|f| !f.is_empty()),
        format!(
            "every row says the folder it sits in ({})",
            folders.join(" | ")
        ),
    );
    if let Some(p) = row_centre(&s, 0) {
        click(app, p);
    }
    hold(ctx, 150);
    key(app, "Ctrl+Shift+K");
    let focused = ctx.until("Find a tab to have the keys", 3_000, &mut || {
        side(app)["find"]["focus"] == json!(true)
    });
    // PT: run.mjs:2399
    ctx.ok(focused, "Ctrl+Shift+K puts the cursor in Find a tab");
    let _ = app.session.type_text("zz-no-such-tab");
    let none_seen = ctx.until("no row to be left", 3_000, &mut || {
        side(app)["seen"] == json!(0)
    });
    // PT: run.mjs:2401
    ctx.ok(none_seen, "a word no tab has leaves no rows");
    let _ = app.session.key("Escape");
    let cleared = ctx.until("Escape to clear the box", 3_000, &mut || {
        let f = side(app);
        f["find"]["value"] == json!("")
            && f["find"]["focus"] == json!(false)
            && f["seen"] == json!(6)
    });
    // PT: run.mjs:2404
    ctx.ok(cleared, "Escape clears it and lets go");
    let target = labels(&side(app))
        .get(1)
        .cloned()
        .unwrap_or_default()
        .trim()
        .to_string();
    key(app, "Ctrl+Shift+K");
    ctx.until("Find a tab to have the keys", 3_000, &mut || {
        side(app)["find"]["focus"] == json!(true)
    });
    let _ = app.session.type_text(&target);
    hold(ctx, 150);
    let seen = num(&side(app)["seen"]) as i64;
    // PT: run.mjs:2411
    ctx.ok(
        (1..6).contains(&seen),
        format!("typing a tab's name keeps the rows that have it ({seen} for \"{target}\")"),
    );
    let _ = app.session.key("Enter");
    let picked = ctx.until("Enter to put the first match in front", 3_000, &mut || {
        let f = side(app);
        let front_label = rows(&f)
            .iter()
            .find(|r| r["front"] == json!(true))
            .and_then(|r| r["label"].as_str().map(|s| s.to_lowercase()))
            .unwrap_or_default();
        f["find"]["value"] == json!("")
            && f["find"]["focus"] == json!(false)
            && f["seen"] == json!(6)
            && front_label.contains(&target.to_lowercase())
    });
    // PT: run.mjs:2415
    ctx.ok(picked, "Enter puts the first match in front and lets go");
    if let Some(p) = row_centre(&side(app), 0) {
        click(app, p);
    }

    // ---- the marks (PT 2417-2449)
    for ind in ["ring", "minimal"] {
        let want: &[&str] = if ind == "ring" {
            &["ring"]
        } else {
            &["bar", "minimal"]
        };
        let set = pick_seg(ctx, app, "agent-indicator", want);
        back_to_first(ctx, app);
        hold(ctx, 250);
        let s = side(app);
        let motions: Vec<String> = rows(&s)
            .iter()
            .map(|r| r["mark"]["motion"].as_str().unwrap_or("").to_string())
            .collect();
        let tag = format!("vertical, {ind}");
        if ind == "ring" {
            let ring_ok = set
                && motions.len() == 6
                && [0usize, 2, 3, 4, 5].iter().all(|i| motions[*i] == "ring")
                && motions[1] == "none";
            // PT: run.mjs:2433
            ctx.ok(
                ring_ok,
                format!("{tag}: one ring on each marked card, nothing else ({motions:?})"),
            );
            let k = scale(app);
            let at_end = snap(ctx, app, "side-ring").is_some_and(|sn| {
                [0usize, 2, 3, 4, 5].iter().all(|i| {
                    let Some(r) = rows(&s).get(*i).and_then(|r| rect_of(&r["rect"])) else {
                        return false;
                    };
                    let bg = sn.at(dv(r.0 + r.2 - 40.0, k), dv(r.1 + 3.0, k));
                    let end = (r.0 + r.2 - 30.0, r.1 + 8.0, 24.0, r.3 - 16.0);
                    ink_box(&sn, k, end, bg, 40).is_some()
                })
            });
            // PT: run.mjs:2434
            ctx.ok(
                at_end,
                format!("{tag}: each ring at the card's end, in the close's place"),
            );
        } else {
            let bar_ok = set
                && motions.len() == 6
                && [(2usize, "run"), (3, "line"), (4, "line"), (5, "line")]
                    .iter()
                    .all(|(i, m)| motions[*i] == *m);
            // PT: run.mjs:2437
            ctx.ok(
                bar_ok,
                format!("{tag}: the strip's Bar marks ({motions:?})"),
            );
        }
    }
    pick_seg(ctx, app, "agent-indicator", &["ring"]);
    back_to_first(ctx, app);

    // ---- a row picks, and drags (PT 2452-2463)
    let s = side(app);
    let second = rows(&s)
        .get(1)
        .map(|r| r["id"].as_str().unwrap_or("").to_string());
    if let Some(p) = row_centre(&s, 1) {
        click(app, p);
    }
    let fronted = ctx.until("a click on a row to put it in front", 3_000, &mut || {
        let f = side(app);
        rows(&f).get(1).is_some_and(|r| {
            r["front"] == json!(true) && Some(r["id"].as_str().unwrap_or("").to_string()) == second
        })
    });
    // PT: run.mjs:2453
    ctx.ok(fronted, "a click on a row puts it in front");
    let s = side(app);
    let before = labels(&s);
    if let Some((cx, cy)) = row_centre(&s, 1) {
        drag(
            app,
            (f64::from(cx) + 40.0, f64::from(cy)),
            (f64::from(cx) + 40.0, f64::from(cy) + 48.0),
            8,
        );
    }
    hold(ctx, 200);
    let after = labels(&side(app));
    // PT: run.mjs:2462
    ctx.ok(
        after.len() > 2 && before.len() > 2 && after[2] == before[1] && after[1] == before[2],
        format!(
            "dragging a row down a row swaps it ({:?} -> {:?})",
            &before[..before.len().min(3)],
            &after[..after.len().min(3)]
        ),
    );

    // ---- collapsed (PT 2469-2540)
    // PT: run.mjs:2467. DROPPED check: "the sidebar has no collapse button". An absent control has
    // nothing a probe or a pixel can name, so the key (checked next) is the only way and it is read.
    ctx.skip("the sidebar has no collapse button: an absent control has nothing to probe; Ctrl+Shift+B below is the only way");
    key(app, "Ctrl+Shift+B");
    let stored_collapsed = ctx.until("Collapsed to be stored", 5_000, &mut || {
        stored(app, "flare.window.sidebar") == "collapsed"
    });
    // PT: run.mjs:2469
    ctx.ok(
        stored_collapsed,
        "Ctrl+Shift+B collapses it, and Collapsed is stored",
    );
    move_to(app, 700.0, 400.0);
    hold(ctx, 600);
    let away = side(app);
    // PT: run.mjs:2472
    ctx.ok(
        away["collapsed"] == json!(true)
            && away["shown"] == json!(false)
            && num(&away["nav"]["right"]) <= 0.0,
        format!("collapsed, the sidebar is out of sight ({})", away["nav"]),
    );
    let grid_away = ui_rect(app, "grid").unwrap_or((0.0, 0.0, 0.0, 0.0));
    // PT: run.mjs:2473
    ctx.ok(
        num(&away["host_left"]) == 0.0 && grid_away.0 < 20.0,
        format!(
            "and the terminal takes the full width (grid at {})",
            grid_away.0
        ),
    );
    let width_away = grid_away.2;
    move_to(app, 3.0, 300.0);
    move_to(app, 2.0, 310.0);
    let shows = ctx.until(
        "the pointer at the left edge to show it",
        3_000,
        &mut || side(app)["shown"] == json!(true),
    );
    // PT: run.mjs:2479
    ctx.ok(shows, "the pointer at the left edge shows it");
    hold(ctx, 400);
    let near_s = side(app);
    // PT: run.mjs:2481
    ctx.ok(
        num(&near_s["nav"]["x"]) == 0.0 && num(&near_s["nav"]["right"]) > 100.0,
        format!("over the terminal ({})", near_s["nav"]),
    );
    let grid_near = ui_rect(app, "grid").unwrap_or((0.0, 0.0, 0.0, 0.0));
    // PT: run.mjs:2482
    ctx.ok(
        (grid_near.2 - width_away).abs() < 0.5 && num(&near_s["host_left"]) == 0.0,
        "and the terminal keeps its size, so the shell is not redrawn",
    );
    move_to(app, 80.0, 320.0);
    hold(ctx, 500);
    // PT: run.mjs:2485
    ctx.ok(
        side(app)["shown"] == json!(true),
        "it stays while the pointer is on it",
    );
    move_to(app, 800.0, 400.0);
    let gone = ctx.until("it to go when the pointer leaves", 3_000, &mut || {
        side(app)["shown"] == json!(false)
    });
    // PT: run.mjs:2489
    ctx.ok(gone, "and goes when the pointer leaves it");
    move_to(app, 60.0, 300.0);
    move_to(app, 20.0, 305.0);
    let twenty = ctx.until("the pointer 20 px in to show it", 3_000, &mut || {
        side(app)["shown"] == json!(true)
    });
    // PT: run.mjs:2493
    ctx.ok(twenty, "the pointer 20 px in from the left shows it");
    move_to(app, 800.0, 400.0);
    ctx.until("it to go", 3_000, &mut || {
        side(app)["shown"] == json!(false)
    });
    move_to(app, 2.0, 300.0);
    ctx.until("it to start to show", 3_000, &mut || {
        side(app)["shown"] == json!(true)
    });
    let right = num(&near_s["nav"]["right"]);
    move_to(app, right - 6.0, 320.0);
    hold(ctx, 1200);
    // PT: run.mjs:2501
    ctx.ok(
        side(app)["shown"] == json!(true),
        "a pointer that got near its far edge before the slide did keeps it shown",
    );
    move_to(app, 800.0, 400.0);
    ctx.until("it to go", 3_000, &mut || {
        side(app)["shown"] == json!(false)
    });
    // PT: run.mjs:2510-2527. The four "main's cursor" checks read the real mouse position, which a
    // run never moves; flare-win::edge_watch holds them in unit tests.
    for what in [
        "main's cursor at the left edge shows it",
        "and it stays with the cursor at its top",
        "and 3 px past its edge",
        "and goes once the cursor is clearly off it",
    ] {
        ctx.skip(&format!(
            "{what}: needs the real mouse, which a run never moves (flare-win::edge_watch tests hold it)"
        ));
    }
    Ok(())
}

/// Checks 42 to 82 of PT's scenario: pinned again, the bar beside the sidebar, the hit tests, the
/// collapsed and slid-out looks, the zoom, Settings under the sidebar, and back to the strip.
fn vertical_b(ctx: &mut Ctx, app: &mut App, gpu: bool) -> Result<(), String> {
    let k = scale(app);
    let screen_before = ui_rect(app, "grid").map(|g| g.2).unwrap_or(0.0);
    key(app, "Ctrl+Shift+B");
    // Read at once, inside the slide and the hold, before anything waits.
    let mid = side(app);
    let screen_mid = ui_rect(app, "grid").map(|g| g.2).unwrap_or(-1.0);
    let repinned = ctx.until("Open to be stored", 5_000, &mut || {
        stored(app, "flare.window.sidebar") == "open"
    });
    // PT: run.mjs:2538
    ctx.ok(repinned, "Ctrl+Shift+B pins it again, and Open is stored");
    // PT: run.mjs:2552
    ctx.ok(
        mid["shown"] == json!(true) && mid["sliding"] == json!(true),
        format!(
            "pinned, the sidebar and its room slide together (sliding {})",
            mid["sliding"]
        ),
    );
    // PT: run.mjs:2553
    ctx.ok(
        (screen_mid - screen_before).abs() < 0.5,
        format!("and the terminal keeps its width while they do ({screen_mid} vs {screen_before})"),
    );
    // PT: run.mjs:2554
    let beside = ctx.until("the terminal to sit beside the sidebar", 3_000, &mut || {
        ui_rect(app, "grid").is_some_and(|g| g.0 > 100.0)
    });
    ctx.ok(beside, "then it sits beside the terminal");
    let fitted = ctx.until("the terminal to fit its new width", 3_000, &mut || {
        ui_rect(app, "grid").is_some_and(|g| g.2 < screen_before - 0.5)
    });
    // PT: run.mjs:2555
    ctx.ok(
        fitted,
        "and the terminal fits its new width once the slide is over",
    );

    // ---- the bar beside the sidebar, always down (PT 2557-2606)
    back_to_first(ctx, app);
    move_to(app, 600.0, 400.0);
    hold(ctx, 400);
    let s = side(app);
    let win_w = num(&s["window"]["w"]);
    let win_h = num(&s["window"]["h"]);
    let nav_w = num(&s["nav"]["w"]);
    let secondary = token(&s, "secondary");
    let icon = token(&s, "icon");
    let list = rows(&s);
    let sn = snap(ctx, app, "beside-bar");
    let bar = &s["bar"];
    // PT: run.mjs:2589
    let bar_ok = num(&bar["y"]) == 0.0
        && num(&bar["h"]) == BAR_H
        && sn.as_ref().is_some_and(|sn| {
            near(
                sn.at(dv(nav_w + 100.0, k), dv(BAR_H / 2.0, k)),
                secondary,
                8,
            ) && !near(
                sn.at(dv(nav_w + 100.0, k), dv(BAR_H + 20.0, k)),
                secondary,
                8,
            )
        });
    ctx.ok(bar_ok, format!("one bar, at the top, always there ({bar})"));
    // PT: run.mjs:2592
    let across = sn.as_ref().is_some_and(|sn| {
        near(sn.at(dv(5.0, k), dv(BAR_H / 2.0, k)), secondary, 8)
            && near(sn.at(dv(win_w - 3.0, k), dv(BAR_H / 2.0, k)), secondary, 8)
    });
    ctx.ok(
        num(&bar["x"]) == 0.0 && num(&bar["w"]) == win_w && across,
        format!(
            "across the whole window, over the sidebar's column too (bar {}px wide)",
            bar["w"]
        ),
    );
    // PT: run.mjs:2593
    let in_bar =
        |r: Option<(f64, f64, f64, f64)>| r.is_some_and(|r| r.1 >= 0.0 && r.1 + r.3 <= BAR_H);
    let cog = ui_rect(app, "cog");
    let close = ui_rect(app, "close");
    ctx.ok(
        in_bar(cog) && in_bar(close) && in_bar(ui_rect(app, "min")) && in_bar(ui_rect(app, "max")),
        "with Settings and the window's buttons",
    );
    let grid = ui_rect(app, "grid").unwrap_or((0.0, 0.0, 0.0, 0.0));
    // PT: run.mjs:2594
    ctx.ok(
        grid.1 > 20.0 && grid.1 <= 40.0,
        format!("the terminal sits below it, nothing covered ({})", grid.1),
    );
    let first_top = list.first().map(|r| num(&r["rect"]["y"])).unwrap_or(0.0);
    let row_pixels = sn.as_ref().is_some_and(|sn| {
        list.first().is_some_and(|r| {
            let x = dv(num(&r["rect"]["x"]) + 120.0, k);
            differs(
                sn.at(x, dv(first_top + 3.0, k)),
                sn.at(x, dv(first_top - 2.0, k)),
            )
        })
    });
    // PT: run.mjs:2595
    ctx.ok(
        num(&s["nav"]["y"]) == BAR_H && (76.0..=100.0).contains(&first_top) && row_pixels,
        format!("the sidebar starts under the bar, its tabs under Find a tab ({}px, rows at {first_top}px)", s["nav"]["y"]),
    );
    let sec_now = token(&s, "secondary");
    let cut_now = sn
        .as_ref()
        .map(|sn| corner_cut(sn, k, nav_w, sec_now))
        .unwrap_or_else(|| Err("no snapshot".into()));
    // PT: run.mjs:2596
    ctx.ok(
        cut_now.is_ok(),
        format!("the terminal is cut round where the bar and the sidebar meet ({cut_now:?})"),
    );
    // PT: run.mjs:2599
    let nt = rect_of(&s["new_tab"]);
    let last_bottom = list
        .last()
        .and_then(|r| rect_of(&r["rect"]))
        .map(|r| r.1 + r.3)
        .unwrap_or(0.0);
    let nt_ok = nt.is_some_and(|r| {
        let fill = sn.as_ref().is_some_and(|sn| {
            differs(
                sn.at(dv(r.0 + 10.0, k), dv(r.1 + r.3 / 2.0, k)),
                sn.at(dv(r.0 - 4.0, k), dv(r.1 + r.3 / 2.0, k)),
            )
        });
        r.1 >= last_bottom && win_h - (r.1 + r.3) <= 12.0 && r.2 >= nav_w - 20.0 && fill
    });
    ctx.ok(
        nt_ok,
        format!(
            "New tab is a button across the sidebar's foot ({})",
            s["new_tab"]
        ),
    );
    // PT: run.mjs:2600
    let wear = sn.as_ref().is_some_and(|sn| {
        near(sn.at(dv(nav_w / 2.0, k), dv(500.0, k)), secondary, 8)
            && near(
                sn.at(dv(nav_w + 100.0, k), dv(BAR_H / 2.0, k)),
                secondary,
                8,
            )
    });
    ctx.ok(
        wear,
        format!("the sidebar and the bar wear the secondary colour ({secondary:?})"),
    );
    // PT: run.mjs:2617. The app's mark alone, no name, in the buttons' ink.
    let mark = sn.as_ref().and_then(|sn| {
        let ink = ink_box(sn, k, (10.0, 4.0, 30.0, 28.0), secondary, 40)?;
        // The most contrasting pixel of the mark is its ink.
        let mut best = secondary;
        for y in dv(ink.1, k)..dv(ink.1 + ink.3, k) {
            for x in dv(ink.0, k)..dv(ink.0 + ink.2, k) {
                let c = sn.at(x, y);
                if dist(c, secondary) > dist(best, secondary) {
                    best = c;
                }
            }
        }
        Some((ink, best))
    });
    ctx.ok(
        mark.is_some_and(|(b, c)| (8.0..=20.0).contains(&b.2) && near(c, icon, 60)),
        format!("the bar shows the app's mark alone, in the buttons' ink ({mark:?})"),
    );
    // PT: run.mjs:2640-2642. PT sends WM_NCHITTEST at four points and reads HTCAPTION (2) or HTCLIENT (1).
    // Flare has no hit test to ask: its bar moves the window through Slint's WindowMoveArea, which asks
    // winit to start Windows' move loop, and a synthetic Drag cannot start it (this window sits at
    // (-32000, -32000) and its origin never changes). A check that only reads "the origin did not
    // change" would pass for a tab and the terminal by accident, so none of the three is made.
    ctx.skip("the bar moves the window: a synthetic Drag cannot start Windows' move loop and Flare has no WM_NCHITTEST to ask");
    ctx.skip("the app's name in the bar moves the window: same reason");
    ctx.skip(
        "a tab and the terminal do not: same reason (an unchanged origin would prove nothing)",
    );
    back_to_first(ctx, app);

    // ---- collapsed: the bar spans, the corner goes (PT 2645-2654)
    key(app, "Ctrl+Shift+B");
    move_to(app, 800.0, 400.0);
    ctx.until("the sidebar to be out of sight", 4_000, &mut || {
        let s = side(app);
        s["collapsed"] == json!(true) && s["shown"] == json!(false)
    });
    hold(ctx, 600);
    let sc = snap(ctx, app, "collapsed");
    // The terminal's own ground, read where no text is (the lower right).
    let inside = sc.as_ref().map(|sn| sn.at(dv(1000.0, k), dv(640.0, k)));
    let square = sc.as_ref().zip(inside).is_some_and(|(sn, inside)| {
        near(sn.at(dv(1.0, k), dv(BAR_H + 1.0, k)), inside, 12)
            && near(sn.at(dv(5.0, k), dv(BAR_H / 2.0, k)), secondary, 8)
            && near(sn.at(dv(win_w - 3.0, k), dv(BAR_H / 2.0, k)), secondary, 8)
    });
    // PT: run.mjs:2647
    ctx.ok(
        square,
        "collapsed, the bar spans the window and the corner goes",
    );
    hold(ctx, 400);
    let parked = sc
        .as_ref()
        .zip(inside)
        .is_some_and(|(sn, inside)| region_clean(sn, k, (0.0, BAR_H + 70.0, 3.0, 400.0), inside));
    // PT: run.mjs:2654
    ctx.ok(parked, "hidden, the curved cut waits off the window's edge");

    // ---- slid out (PT 2655-2668)
    move_to(app, 2.0, 400.0);
    ctx.until("the sidebar to show", 3_000, &mut || {
        side(app)["shown"] == json!(true)
    });
    hold(ctx, 400);
    let sl = side(app);
    let cs = probe(app, "corner.state", Value::Null);
    let slid_snap = snap(ctx, app, "slid-out");
    let w_now = num(&sl["nav"]["w"]);
    let slid_ok = slid_snap
        .as_ref()
        .map(|sn| corner_cut(sn, k, w_now, secondary))
        .unwrap_or_else(|| Err("no snapshot".into()));
    // PT: run.mjs:2662
    ctx.ok(
        slid_ok.is_ok() && (num(&cs["x"]) - w_now).abs() <= 1.0 && num(&cs["y"]) == BAR_H,
        format!("slid out, a curved corner joins it to the bar ({slid_ok:?})"),
    );
    // PT: run.mjs:2666
    let bar_free = slid_snap.as_ref().is_some_and(|sn| {
        near(
            sn.at(dv(w_now / 2.0 + 30.0, k), dv(BAR_H / 2.0, k)),
            secondary,
            8,
        )
    });
    ctx.ok(
        num(&sl["nav"]["y"]) == BAR_H && bar_free,
        format!(
            "slid out, it starts under the bar, never over it ({}px)",
            sl["nav"]["y"]
        ),
    );
    move_to(app, 800.0, 400.0);
    hold(ctx, 500);
    key(app, "Ctrl+Shift+B");
    ctx.until("the sidebar pinned", 4_000, &mut || {
        side(app)["open"] == json!(true)
    });
    hold(ctx, 300);

    // ---- see-through, one coat everywhere (PT 2669-2689)
    // PT: run.mjs:2696, 2699
    ctx.skip(&format!(
        "see-through: the sidebar, the bar and the terminal each one coat: {}",
        if gpu {
            "not checked yet on the GPU renderer"
        } else {
            SOFTWARE_SKIP
        }
    ));
    ctx.skip(&format!(
        "see-through: the same line: {}",
        if gpu {
            "not checked yet on the GPU renderer"
        } else {
            SOFTWARE_SKIP
        }
    ));

    // ---- at 125 percent, a 235 px sidebar's edge lands on a whole screen pixel (PT 2690-2725)
    let zoomed = send(
        app,
        Request::DpiChange {
            dpi: DPI_125,
            x: None,
            y: None,
        },
    ) && ctx.until("125 percent", 6_000, &mut || {
        let k = scale(app);
        k > 1.2 && k < 1.3
    });
    hold(ctx, 400);
    let kz = scale(app);
    let sz = side(app);
    let g = rect_of(&sz["grip"]);
    let wz = num(&sz["nav"]["w"]);
    // PT: run.mjs:2718. The grip is the thing a press at the sidebar's edge lands on.
    ctx.ok(
        zoomed && g.is_some_and(|g| (g.0 + g.2 / 2.0 - wz).abs() <= 0.5 && g.2 >= 5.0),
        format!(
            "the resize grip is on top at the sidebar's edge ({})",
            sz["grip"]
        ),
    );
    let stored_before = stored(app, "flare.window.sidebarWidth");
    if let Some(g) = g {
        let (gx, gy) = (g.0 + g.2 / 2.0, g.1 + 300.0);
        drag(app, (gx, gy), (gx + 12.0, gy), 5);
    }
    move_to(app, 700.0, 500.0);
    hold(ctx, 400);
    let changed = ctx.until("the width to be stored", 5_000, &mut || {
        stored(app, "flare.window.sidebarWidth") != stored_before
    });
    let sw = side(app);
    // PT: run.mjs:2727
    ctx.ok(
        changed,
        format!(
            "dragging the grip resizes the sidebar and stores it ({} was {stored_before})",
            stored(app, "flare.window.sidebarWidth")
        ),
    );
    let w_css = num(&sw["nav"]["w"]);
    let dev = w_css * kz;
    let edge_snap = snap(ctx, app, "edge-125");
    let whole = (dev - dev.round()).abs() < 0.01
        && edge_snap.as_ref().is_some_and(|sn| {
            let y = dv(500.0, kz);
            let col = dev.round() as i64;
            near(sn.at(col - 1, y), secondary, 8) && !near(sn.at(col, y), secondary, 8)
        });
    // PT: run.mjs:2728
    ctx.ok(
        kz.fract() != 0.0 && whole,
        format!("at a scale that is not whole the sidebar meets the terminal on a whole screen pixel ({w_css} css px, {dev:.2} device px)"),
    );
    send(
        app,
        Request::DpiChange {
            dpi: 96,
            x: None,
            y: None,
        },
    );
    ctx.until("100 percent", 6_000, &mut || scale(app) < 1.1);
    hold(ctx, 300);
    // Back to the width it had.
    let k = scale(app);
    let s = side(app);
    if let Some(g) = rect_of(&s["grip"]) {
        let want = num(&s["nav"]["w"]) - 12.0;
        let (gx, gy) = (g.0 + g.2 / 2.0, g.1 + 300.0);
        drag(app, (gx, gy), (gx - 12.0, gy), 5);
        let _ = want;
    }
    hold(ctx, 300);

    // ---- slid out: nothing of the page lies above it (PT 2735-2776)
    key(app, "Ctrl+Shift+B");
    move_to(app, 800.0, 400.0);
    hold(ctx, 600);
    move_to(app, 2.0, 400.0);
    ctx.until("the sidebar to show", 3_000, &mut || {
        side(app)["shown"] == json!(true)
    });
    hold(ctx, 400);
    let s = side(app);
    let nav_w = num(&s["nav"]["w"]);
    let rows_end = BAR_H + 50.0 + (rows(&s).len() as f64) * 50.0 + 12.0;
    let foot = num(&s["window"]["h"]) - 60.0;
    let region = (2.0, rows_end, nav_w - 12.0, foot - rows_end);
    let clean =
        snap(ctx, app, "slid-clean").is_some_and(|sn| region_clean(&sn, k, region, secondary));
    // PT: run.mjs:2751
    ctx.ok(
        clean,
        "slid out, nothing of the page beside it lies above it",
    );
    // And over Settings, whose search box and magnifier are raised.
    move_to(app, 800.0, 400.0);
    hold(ctx, 500);
    let opened = open_settings(ctx, app);
    move_to(app, 2.0, 400.0);
    ctx.until("the sidebar to show", 3_000, &mut || {
        side(app)["shown"] == json!(true)
    });
    hold(ctx, 400);
    let s = side(app);
    let rows_end = BAR_H + 50.0 + (rows(&s).len() as f64) * 50.0 + 12.0;
    let region = (2.0, rows_end, num(&s["nav"]["w"]) - 12.0, foot - rows_end);
    let over = snap(ctx, app, "slid-over-settings")
        .is_some_and(|sn| region_clean(&sn, k, region, secondary));
    // PT: run.mjs:2775
    ctx.ok(
        opened && over,
        "slid out over Settings, the sidebar covers its search icon",
    );
    back_to_first(ctx, app);
    move_to(app, 2.0, 400.0);
    ctx.until("the sidebar to show", 3_000, &mut || {
        side(app)["shown"] == json!(true)
    });
    hold(ctx, 300);
    // PT: run.mjs:2790
    ctx.skip(&format!(
        "collapsed and slid out, the sidebar is the window's acrylic: {}",
        if gpu {
            "not checked yet on the GPU renderer"
        } else {
            SOFTWARE_SKIP
        }
    ));
    move_to(app, 800.0, 400.0);
    hold(ctx, 500);
    key(app, "Ctrl+Shift+B");
    ctx.until("the sidebar pinned", 4_000, &mut || {
        side(app)["open"] == json!(true)
    });
    back_to_first(ctx, app);

    // ---- back to the strip (PT 2800-2815)
    let to_h = pick_seg(ctx, app, "tab-layout", &["horizontal"]);
    back_to_first(ctx, app);
    let strip_back = to_h
        && ctx.until("the strip to come back", 3_000, &mut || {
            let s = side(app);
            s["strip"] == json!(true) && s["open"] == json!(false)
        });
    // PT: run.mjs:2806
    ctx.ok(strip_back, "Horizontal puts the strip back");
    hold(ctx, 300);
    let mark_rect = ui_rect(app, "mark");
    let first_tab = tabs_of(app)
        .first()
        .and_then(|t| rect_of(&t["rect"]))
        .map(|r| r.0)
        .unwrap_or(-1.0);
    let mark_snap = snap(ctx, app, "strip-mark");
    let strip_bg = token(&side(app), "ground");
    let paths = mark_snap.as_ref().zip(mark_rect).and_then(|(sn, m)| {
        let bg = sn.at(dv(2.0, k), dv(2.0, k));
        let _ = strip_bg;
        ink_box(sn, k, (m.0 + 4.0, m.1 + 4.0, m.2 - 8.0, m.3 - 8.0), bg, 40)
    });
    // PT: run.mjs:2810
    ctx.ok(
        paths.is_some_and(|b| (8.0..=20.0).contains(&b.2)) && mark_rect.is_some_and(|m| m.0 < first_tab),
        format!("with the strip, the same quiet mark sits before the tabs ({mark_rect:?}, first tab at {first_tab})"),
    );
    // With the strip, Ctrl+Shift+B is the shell's again.
    // The settings file is written a moment after a change, so wait for the pin from above to land
    // before taking the "before" reading; a stale "collapsed" would blame the chord for that write.
    ctx.until("the pinned mode is stored", 4_000, &mut || {
        stored(app, "flare.window.sidebar") == "open"
    });
    let mode_before = stored(app, "flare.window.sidebar");
    key(app, "Ctrl+Shift+B");
    hold(ctx, 400);
    // PT: run.mjs:2814
    ctx.ok(
        stored(app, "flare.window.sidebar") == "open" && mode_before == "open",
        format!(
            "with the strip, Ctrl+Shift+B does not touch the sidebar ({mode_before:?} then {:?})",
            stored(app, "flare.window.sidebar")
        ),
    );
    Ok(())
}
