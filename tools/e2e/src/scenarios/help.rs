//! The command help panel scenario (Task 37): helpPanel (PT: run.mjs:4614-5088, 89 assertions).
//!
//! Flare answers through `help.state` (the bridge the panel draws from: rows, chips, query, the
//! words a screen reader hears, the sheet's width and box), `app.state` (tabs, the menu, the
//! dialog), `clip.last`, `settings.row`, and snapshots read off the disk for what only pixels tell
//! (the zebra, the heights of rows, the corners, the copy button's box, the selection's colour).
//! The layout checks run at 100 percent and again at 225 percent (the second pass is
//! `second_pass_225`). Where Flare differs from the page, a comment says how the check was adapted.

use std::path::Path;
use std::time::{Duration, Instant};

use flare_e2e_proto::{Button, PointerAction, Request};
use serde_json::{json, Value};

use crate::ctx::{App, Ctx, Launch};
use crate::pixels::{contrast, lum, Rgb};
use crate::world::{mute_psreadline_paste, prompt_timeout, rows_text, wait_for_prompt, Shell};

/// `ctx.ok` with the condition worked out first, so a condition may borrow the context itself
/// (`check!(ctx, opened(ctx, app), "...")`).
macro_rules! check {
    ($ctx:expr, $cond:expr, $msg:expr $(,)?) => {{
        let verdict = $cond;
        $ctx.ok(verdict, $msg);
    }};
}
/// A css rectangle: x, y, width, height.
type R = (f64, f64, f64, f64);

/// The title bar's height in css pixels.
const BAR_H: f64 = 36.0;
/// 225 percent (96 is 100 percent).
const DPI_225: u32 = 216;
/// The question the scenario asks for most of its checks.
const BIG_FILES: &str = "how do I find big files";

// ---- small helpers -----------------------------------------------------------------------------------

fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// `help.state`, `Null` when the probe fails (a check on it then reads false).
fn hs(app: &mut App) -> Value {
    probe(app, "help.state", Value::Null)
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

fn scale_of(app: &mut App) -> f64 {
    app.session
        .state()
        .map(|s| f64::from(s.scale))
        .unwrap_or(1.0)
}

fn tab_count(app: &mut App) -> usize {
    app.session.state().map(|s| s.tabs.len()).unwrap_or(0)
}

fn front_id(app: &mut App) -> String {
    app.session
        .state()
        .ok()
        .and_then(|s| s.front)
        .unwrap_or_default()
}

fn is_open(app: &mut App) -> bool {
    hs(app)["open"] == json!(true)
}

fn rows_json(v: &Value) -> Vec<Value> {
    v["rows"].as_array().cloned().unwrap_or_default()
}

fn first_id(app: &mut App) -> String {
    let v = hs(app);
    rows_json(&v)
        .first()
        .and_then(|r| r["id"].as_str().map(str::to_string))
        .unwrap_or_default()
}

fn clip(app: &mut App) -> String {
    probe(app, "clip.last", Value::Null)["text"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

fn marked_at(app: &mut App) -> Option<usize> {
    let v = hs(app);
    rows_json(&v)
        .iter()
        .position(|r| r["marked"] == json!(true))
}

fn click_at(app: &mut App, x: f64, y: f64) {
    let _ = app.session.click(x as f32, y as f32);
}

fn click_rect(app: &mut App, r: R) {
    let (x, y) = centre(r);
    let _ = app.session.click(x, y);
}

fn move_to(app: &mut App, x: f64, y: f64) {
    let _ = app.session.call(&Request::Pointer {
        x: x as f32,
        y: y as f32,
        action: PointerAction::Move,
    });
}

fn wheel(app: &mut App, x: f64, y: f64, dy: f32) {
    let _ = app.session.call(&Request::Pointer {
        x: x as f32,
        y: y as f32,
        action: PointerAction::Wheel { dx: 0.0, dy },
    });
}

fn drag(app: &mut App, from: (f64, f64), to: (f64, f64), steps: u32) {
    // The list is a Flickable: it holds a press back (100 ms) to see whether it becomes a scroll,
    // so a drag that takes no time never reaches the text under it. A person's drag takes time:
    // press, hold a moment, then move.
    let at = |x: f64, y: f64, action: PointerAction| Request::Pointer {
        x: x as f32,
        y: y as f32,
        action,
    };
    let _ = app.session.call(&at(from.0, from.1, PointerAction::Move));
    let _ = app.session.call(&at(
        from.0,
        from.1,
        PointerAction::Down {
            button: Button::Left,
        },
    ));
    // e2e pacing: a held press, in 50 ms steps (the runner's rule: no wait longer than one step).
    for _ in 0..3 {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let steps = steps.max(1);
    for i in 1..=steps {
        let f = f64::from(i) / f64::from(steps);
        let (x, y) = (from.0 + (to.0 - from.0) * f, from.1 + (to.1 - from.1) * f);
        let _ = app.session.call(&at(x, y, PointerAction::Move));
        std::thread::sleep(std::time::Duration::from_millis(15)); // e2e pacing: a moving hand
    }
    let _ = app.session.call(&at(
        to.0,
        to.1,
        PointerAction::Up {
            button: Button::Left,
        },
    ));
}

/// The panel opened (the flag is on at once; the slide is `slid`).
fn opened(ctx: &mut Ctx, app: &mut App) -> bool {
    ctx.until("the panel to open", 6_000, &mut || is_open(app))
}

/// The panel opened and its slide and the rows' rise are over.
fn slid(ctx: &mut Ctx, app: &mut App) -> bool {
    ctx.until("the panel to finish sliding in", 8_000, &mut || {
        hs(app)["slid"] == json!(true)
    })
}

fn closed(ctx: &mut Ctx, app: &mut App) -> bool {
    ctx.until("the panel to close", 4_000, &mut || !is_open(app))
}

fn field_focused(app: &mut App) -> bool {
    hs(app)["field_focused"] == json!(true)
}

/// True when `cond` never holds for `ms` (a "nothing happened" proof; polled every 50 ms at most).
fn never(ctx: &mut Ctx, app: &mut App, ms: u64, cond: &mut dyn FnMut(&mut App) -> bool) -> bool {
    let mut left = ms;
    while left > 0 {
        if cond(app) {
            return false;
        }
        ctx.settle(50);
        left = left.saturating_sub(50);
    }
    !cond(app)
}

/// Gives the search field the keyboard (a click in it) and replaces its text with `q`.
fn set_query(app: &mut App, q: &str) {
    let v = hs(app);
    if let Some(r) = rect_of(&v["field"]) {
        click_rect(app, r);
    }
    let have = v["query"].as_str().unwrap_or("").chars().count();
    let _ = app.session.key(crate::chord::FIELD_END);
    for _ in 0..have {
        let _ = app.session.key("Backspace");
    }
    let _ = app.session.type_text(q);
}

/// Waits for the question to be what the field holds and the first answer to be `id`.
fn asked(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    ctx.until(&format!("{id} to answer first"), 4_000, &mut || {
        first_id(app) == id
    })
}

/// Types `text` into the front shell and presses Enter.
fn type_line(app: &mut App, text: &str) {
    let _ = app.session.type_text(text);
    let _ = app.session.key("Enter");
}

fn term_text(app: &mut App) -> String {
    let id = front_id(app);
    rows_text(app, &id)
}

/// The grid's box (the terminal) from `ui.rect`.
fn grid_rect(app: &mut App) -> R {
    rect_of(&probe(app, "ui.rect", json!({ "name": "grid" })))
        .unwrap_or((0.0, BAR_H, 1000.0, 600.0))
}

/// Clicks inside the terminal, to give it the keyboard.
fn click_terminal(app: &mut App) {
    let g = grid_rect(app);
    click_at(app, g.0 + 40.0, g.1 + 40.0);
}

fn rgb_of_hex(hex: &str) -> Rgb {
    let h = hex.trim_start_matches('#');
    let ch = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    [ch(0), ch(2), ch(4)]
}

/// The snapshot `name` once two taken in a row agree (a fade has run its course): waits with `until`, so a
/// picture that never holds still is a timeout on the next check, never a silent pass.
fn still_shot(ctx: &mut Ctx, app: &mut App, name: &str) -> Option<Img> {
    let (dir, scenario) = (ctx.shots.clone(), ctx.name);
    let mut last: Option<Img> = None;
    ctx.until(
        &format!("{name}: the picture holds still"),
        3_000,
        &mut || {
            let now = crate::ctx::shot_file(&dir, scenario, name, |p| app.session.snapshot(p))
                .and_then(|p| Img::load(&p));
            let same = now.is_some() && now == last;
            last = now;
            same
        },
    );
    last
}

// ---- snapshots read off the disk ---------------------------------------------------------------------

/// A decoded snapshot: device pixels, three bytes each.
#[derive(PartialEq)]
pub struct Img {
    pub w: usize,
    pub h: usize,
    px: Vec<u8>,
}

impl Img {
    /// Decodes the PNG at `path` (RGB or RGBA).
    pub fn load(path: &Path) -> Option<Img> {
        let file = std::fs::File::open(path).ok()?;
        let mut reader = png::Decoder::new(std::io::BufReader::new(file))
            .read_info()
            .ok()?;
        let mut buf = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buf).ok()?;
        let step = match info.color_type {
            png::ColorType::Rgba => 4,
            png::ColorType::Rgb => 3,
            _ => return None,
        };
        let px: Vec<u8> = buf[..info.buffer_size()]
            .chunks_exact(step)
            .flat_map(|c| [c[0], c[1], c[2]])
            .collect();
        Some(Img {
            w: info.width as usize,
            h: info.height as usize,
            px,
        })
    }

    pub fn at(&self, x: usize, y: usize) -> Rgb {
        if x >= self.w || y >= self.h {
            return [0, 0, 0];
        }
        let i = (y * self.w + x) * 3;
        [self.px[i], self.px[i + 1], self.px[i + 2]]
    }

    #[cfg(test)]
    fn solid(w: usize, h: usize, c: Rgb) -> Img {
        Img {
            w,
            h,
            px: (0..w * h).flat_map(|_| c).collect(),
        }
    }

    #[cfg(test)]
    fn put(&mut self, x: usize, y: usize, c: Rgb) {
        let i = (y * self.w + x) * 3;
        self.px[i..i + 3].copy_from_slice(&c);
    }
}

/// The runs of a column: `(is_plain, start, end)` with `end` exclusive, where plain is `plain`.
pub fn runs(col: &[Rgb], plain: Rgb) -> Vec<(bool, usize, usize)> {
    let mut out: Vec<(bool, usize, usize)> = Vec::new();
    for (i, c) in col.iter().enumerate() {
        let p = *c == plain;
        match out.last_mut() {
            Some(last) if last.0 == p => last.2 = i + 1,
            _ => out.push((p, i, i + 1)),
        }
    }
    out
}

/// The box of the pixels whose colour differs between two snapshots by `thr` or more in any channel,
/// inside `region` (x0, y0, x1, y1, end exclusive), as (x0, y0, x1, y1) with the end exclusive.
pub fn diff_bbox(
    a: &Img,
    b: &Img,
    region: (usize, usize, usize, usize),
    thr: u8,
) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in region.1..region.3.min(a.h).min(b.h) {
        for x in region.0..region.2.min(a.w).min(b.w) {
            let (p, q) = (a.at(x, y), b.at(x, y));
            if (0..3).any(|i| p[i].abs_diff(q[i]) >= thr) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x1 > x0).then_some((x0, y0, x1, y1))
}

/// The most common colour of `b` among the pixels that differ from `a` inside `region`.
pub fn changed_mode(
    a: &Img,
    b: &Img,
    region: (usize, usize, usize, usize),
    thr: u8,
) -> Option<Rgb> {
    let mut seen: Vec<(Rgb, usize)> = Vec::new();
    for y in region.1..region.3.min(a.h).min(b.h) {
        for x in region.0..region.2.min(a.w).min(b.w) {
            let (p, q) = (a.at(x, y), b.at(x, y));
            if (0..3).any(|i| p[i].abs_diff(q[i]) >= thr) {
                match seen.iter_mut().find(|(c, _)| *c == q) {
                    Some(e) => e.1 += 1,
                    None => seen.push((q, 1)),
                }
            }
        }
    }
    seen.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c)
}

/// What a measured list looks like: the css rows of the first four answers and the columns.
pub struct Look {
    pub k: f64,
    pub list: R,
    pub plain: Rgb,
    pub stripe: Rgb,
    /// The css y range of rows 0 to 3 (row 0 starts at the list's top).
    pub rows: Vec<(f64, f64)>,
    /// The css x where the command well starts, per row 1 to 3.
    pub well_x: Vec<f64>,
    /// The css x of the first ink of the name in row 1, from the row's left edge.
    pub name_pad: f64,
    pub shot: Img,
}

impl Look {
    pub fn height(&self, i: usize) -> f64 {
        self.rows.get(i).map_or(0.0, |r| r.1 - r.0)
    }
}

/// Takes a snapshot and reads the list's first four rows off it. The pointer must be off the list
/// and no row marked, or a row's fill is not the plain or the stripe.
fn measure(ctx: &mut Ctx, app: &mut App, name: &str) -> Option<Look> {
    settle_fade(ctx);
    let v = hs(app);
    let list = rect_of(&v["list"])?;
    let panel = rect_of(&v["panel"])?;
    let k = scale_of(app);
    let path = ctx.shot(app, name)?;
    let shot = Img::load(&path)?;
    let dev = |css: f64| (css * k).round() as usize;
    let plain = shot.at(dev(panel.0 + 6.0), dev(BAR_H + 3.0));
    let x = dev(list.0 + 3.0);
    let (y0, y1) = (dev(list.1), dev(list.1 + list.3.min(420.0)));
    let col: Vec<Rgb> = (y0..y1).map(|y| shot.at(x, y)).collect();
    let rs = runs(&col, plain);
    if rs.len() < 4 || !rs[0].0 || rs[1].0 || !rs[2].0 || rs[3].0 {
        return None;
    }
    let stripe = col[rs[1].1];
    let to_css = |i: usize| (y0 + i) as f64 / k;
    let mut rows = vec![(list.1, to_css(rs[0].2))];
    for r in &rs[1..4] {
        rows.push((to_css(r.1), to_css(r.2)));
    }
    // The command well's left edge: walk left from inside it to the row's own fill.
    let mut well_x = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(1) {
        let bg = if i % 2 == 0 { plain } else { stripe };
        let y = dev((row.0 + row.1) / 2.0);
        // Six bg pixels in a row end the well: a glyph's edge can be the row's own colour by chance.
        let mut xx = dev(list.0 + list.2 - 20.0);
        while xx > dev(list.0) + 6 && (0..6).any(|d| shot.at(xx - d, y) != bg) {
            xx -= 1;
        }
        well_x.push((xx + 1) as f64 / k);
    }
    // The name's first ink in row 1, scanning a few lines of the row.
    let mut ink = f64::MAX;
    let (ra, rb) = (dev(rows[1].0) + 3, dev(rows[1].1).saturating_sub(3));
    for y in (ra..rb).step_by(2) {
        for xx in dev(list.0)..dev(list.0 + 80.0) {
            if shot.at(xx, y) != stripe {
                ink = ink.min(xx as f64 / k - list.0);
                break;
            }
        }
    }
    Some(Look {
        k,
        list,
        plain,
        stripe,
        rows,
        well_x,
        name_pad: ink,
        shot,
    })
}

/// The css centre y of row `i` of the measured list (rows 0 to 3).
fn row_mid(l: &Look, i: usize) -> f64 {
    (l.rows[i].0 + l.rows[i].1) / 2.0
}

/// The css x of a row's copy button's centre: 16 from the row's right edge, 1 hairline and 13 in.
fn copy_x(l: &Look) -> f64 {
    l.list.0 + l.list.2 - 16.0 - 1.0 - 1.0 - 13.0
}

/// Waits out the 120 ms fade of a row's fill (three short holds; a "nothing changes" wait, not a poll).
fn settle_fade(ctx: &mut Ctx) {
    for _ in 0..3 {
        ctx.settle(50);
    }
}

/// Waits out the slide of the sheet after its width changed (its x animates over 460 ms; a
/// "nothing changes" wait, not a poll).
fn settle_slide(ctx: &mut Ctx) {
    for _ in 0..11 {
        ctx.settle(50);
    }
}

/// Moves the pointer off the list (onto the search field) so no row is hovered.
fn park(app: &mut App) {
    let v = hs(app);
    if let Some(r) = rect_of(&v["field"]) {
        let (x, y) = centre(r);
        move_to(app, f64::from(x), f64::from(y));
    }
}

/// Clicks the copy button of row `i` of a measured list.
fn click_copy(app: &mut App, l: &Look, i: usize) {
    click_at(app, copy_x(l), row_mid(l, i));
}

/// Clicks the name of row `i` of a measured list.
fn click_name(app: &mut App, l: &Look, i: usize) {
    click_at(app, l.list.0 + 40.0, row_mid(l, i));
}

// ---- the scenario ------------------------------------------------------------------------------------

/// The panel, from the title-bar button to the close question. PT: run.mjs:4614-5088.
pub fn help_panel(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let mut app = ctx.launch(Launch {
        folders: vec![alpha],
        renderer: Some("software"),
        scale_dpi: Some(96),
        ..Launch::default()
    })?;
    // The window is never focused in the e2e (it may not take the foreground): its own focus event is
    // sent, as a person returning to it would, so a text field shows its selection and its caret.
    let _ = app.session.call(&Request::Focus { on: true });
    let started = Instant::now();
    if !ctx.until("the one tab", 15_000, &mut || tab_count(&mut app) == 1) {
        app.close();
        return Ok(());
    }
    let result = help_a(ctx, &mut app).and_then(|()| help_b(ctx, &mut app, started));
    let profile = app.profile.clone();
    app.close();
    // The first pass ends with an agent tab open; the second starts from the folder alone, not from a
    // restored agent (this profile is the scenario's own).
    let _ = std::fs::remove_file(profile.join("tabs.json"));
    result?;
    second_pass_225(ctx)
}

/// Part A: the button, F1, browsing and searching, the layout, the width and the first copies.
fn help_a(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    let front = front_id(app);
    wait_for_prompt(
        app,
        &front,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    );
    type_line(app, "echo help-$(40+2)-ready");
    // PT: run.mjs:4652
    let echoed = ctx.until("the echo", 20_000, &mut || {
        term_text(app).contains("help-42-ready")
    });
    check!(ctx, echoed, "a shell is running in front");
    // The prompt has to be back before the text is taken as the baseline.
    let front = front_id(app);
    wait_for_prompt(
        app,
        &front,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    );
    let term_before = term_text(app);
    ctx.settle(50);
    // The baseline is kept in the next part, which compares against it.
    let _ = std::fs::write(app.profile.join("help-term-before.txt"), &term_before);

    /* ----- the button ----- */
    // PT: run.mjs:4662
    let st = hs(app);
    check!(
        ctx,
        st["button"] == json!(true),
        "the ? is in the title bar (the setting is on by default)",
    );
    let help_btn = rect_of(&probe(app, "ui.rect", json!({ "name": "help" })));
    if let Some(r) = help_btn {
        click_rect(app, r);
    }
    // PT: run.mjs:4664
    check!(ctx, opened(ctx, app), "a click on it opens the panel");
    slid(ctx, app);
    // PT: run.mjs:4683. A sheet on the right under the title bar, the terminal beside it, no scrim.
    let st = hs(app);
    let panel = rect_of(&st["panel"]).unwrap_or((0.0, 0.0, 0.0, 0.0));
    let page = rect_of(&st["page"]).unwrap_or((0.0, 0.0, 0.0, 0.0));
    let win_w = page.0 + page.2;
    let win_h = page.1 + page.3;
    let grid = grid_rect(app);
    let sheet_ok = (win_w - (panel.0 + panel.2)).abs() <= 1.0
        && panel.1 >= BAR_H - 1.0
        && (win_h - (panel.1 + panel.3)).abs() <= 2.0
        && panel.0 - grid.0 > 200.0;
    check!(
        ctx,
        sheet_ok,
        format!(
            "it is a sheet on the right under the title bar, the terminal beside it, no scrim ({panel:?} in {page:?})"
        ),
    );
    // PT: run.mjs:4687
    check!(
        ctx,
        ctx.until("the field to take the focus", 3_000, &mut || field_focused(
            app
        )),
        "and the search field has the focus",
    );
    // PT: run.mjs:4688
    let st = hs(app);
    check!(
        ctx,
        st["shell"] == json!("powershell"),
        "the chip is the shell of the tab in front (PowerShell)",
    );
    let headers: Vec<String> = st["categories"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| c.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let entries = rows_json(&st).len();
    let placeholder = st["placeholder"].as_str().unwrap_or("").to_string();
    // PT: run.mjs:4695
    check!(
        ctx,
        headers
            .first()
            .is_some_and(|h| h.eq_ignore_ascii_case("folders"))
            && headers.len() >= 2,
        format!("with no question it browses by category ({headers:?})"),
    );
    // PT: run.mjs:4696
    check!(
        ctx,
        (20..=200).contains(&entries),
        format!(
            "and draws a first page, not the whole catalogue ({entries} rows; \"{}\")",
            st["count_text"].as_str().unwrap_or("")
        ),
    );
    // PT: run.mjs:4697
    check!(
        ctx,
        placeholder.contains("find big files") && placeholder.contains("port 3000"),
        "the placeholder teaches by example",
    );
    ctx.shot(app, "help-browse-dark");
    // The rest of the list arrives as it is scrolled to: Git is far down.
    let list = rect_of(&st["list"]).unwrap_or((0.0, 0.0, 600.0, 400.0));
    let more = ctx.until("more rows to arrive", 8_000, &mut || {
        wheel(app, list.0 + list.2 / 2.0, list.1 + list.3 / 2.0, -2000.0);
        rows_json(&hs(app)).len() > entries
    });
    // PT: run.mjs:4701
    check!(ctx, more, "scrolling to the end draws more of it");
    let _ = app.session.key("Escape");
    // PT: run.mjs:4706
    check!(ctx, closed(ctx, app), "Escape closes it");

    /* ----- F1, from inside a focused shell ----- */
    click_terminal(app);
    let _ = app.session.key("F1");
    // PT: run.mjs:4711
    check!(
        ctx,
        opened(ctx, app),
        "F1 opens it over a FOCUSED shell (xterm yields the key)"
    );
    // PT: run.mjs:4712
    check!(
        ctx,
        ctx.until("the field to take the focus", 3_000, &mut || field_focused(
            app
        )),
        "and the search field has the focus again",
    );
    slid(ctx, app);
    let _ = app.session.type_text(BIG_FILES);
    // PT: run.mjs:4714
    let first = asked(ctx, app, "ps-biggest-files");
    check!(
        ctx,
        first,
        format!(
            "a plain question finds the PowerShell answer first ({})",
            first_id(app)
        )
    );
    ctx.shot(app, "help-results-dark");
    let bash = pick_chip(ctx, app, "bash");
    // PT: run.mjs:4717
    check!(
        ctx,
        bash && asked(ctx, app, "sh-biggest-files"),
        format!(
            "the Bash chip changes the first answer to the bash one ({})",
            first_id(app)
        ),
    );
    pick_chip(ctx, app, "powershell");
    asked(ctx, app, "ps-biggest-files");

    /* ----- the layout, measured ----- */
    layout_checks(ctx, app, "");

    /* ----- the width is the user's ----- */
    edge_and_width(ctx, app)?;

    /* ----- copy ----- */
    copy_checks(ctx, app)
}

/// Clicks the chip `id`: the chips are left to right and their widths follow their words, so the
/// row is walked from the right of the lit chip until the state says `id` is lit.
fn pick_chip(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    let lit = |app: &mut App| hs(app)["shell"] == json!(id);
    if lit(app) {
        return true;
    }
    let v = hs(app);
    let Some(row) = rect_of(&v["chip_row"]) else {
        return false;
    };
    let y = row.1 + row.3 / 2.0;
    let mut x = row.0 + 8.0;
    while x < row.0 + row.2.min(400.0) {
        click_at(app, x, y);
        ctx.settle(25);
        if lit(app) {
            return true;
        }
        x += 14.0;
    }
    false
}

/// The measured layout (PT: run.mjs:4723-4783). `tag` ends every message (the 225 percent pass).
fn layout_checks(ctx: &mut Ctx, app: &mut App, tag: &str) {
    park(app);
    let st = hs(app);
    let panel = rect_of(&st["panel"]).unwrap_or((0.0, 0.0, 0.0, 0.0));
    let page = rect_of(&st["page"]).unwrap_or((0.0, 0.0, 0.0, 0.0));
    let look = measure(ctx, app, &format!("help-layout{}", tag.trim()));
    let k = scale_of(app);
    // PT: run.mjs:4767
    check!(
        ctx,
        panel.2 >= 600.0 && panel.2 <= 640.0 && panel.3 >= 380.0,
        format!(
            "the panel is 620 wide by default and the terminal's height ({}x{}){tag}",
            panel.2, panel.3
        ),
    );
    // PT: run.mjs:4768
    let inside = panel.1 >= 0.0
        && panel.1 + panel.3 <= page.1 + page.3 + 1.0
        && panel.0 >= 0.0
        && panel.0 + panel.2 <= page.0 + page.2 + 1.0;
    check!(
        ctx,
        inside,
        format!("wholly on screen ({panel:?} in {page:?}){tag}")
    );
    // PT: run.mjs:4769. The panel's own head is the opaque surface: its pixel is the ground, whole.
    let ground = rgb_of_hex(st["ink"]["ground"].as_str().unwrap_or("#000000ff"));
    let ground_a = u8::from_str_radix(
        st["ink"]["ground"]
            .as_str()
            .unwrap_or("")
            .get(7..9)
            .unwrap_or("00"),
        16,
    )
    .unwrap_or(0);
    let head_px = look.as_ref().map(|l| l.plain);
    let opaque =
        ground_a == 255 && head_px.is_some_and(|p| (0..3).all(|i| p[i].abs_diff(ground[i]) <= 2));
    check!(
        ctx,
        opaque,
        format!("on the opaque surface (alpha {ground_a}, head {head_px:?} vs {ground:?}){tag}")
    );
    let Some(l) = look else {
        for what in [
            "a row has its padding",
            "the copy button is big enough to hit",
            "and sits at the right edge of the row",
            "a row is one or two lines, never taller",
            "its two columns start at one x each",
            "square cornered",
            "the rows alternate in colour, the first row plain",
        ] {
            check!(
                ctx,
                false,
                format!(
                    "{what}: the list could not be measured (no plain, stripe, plain, stripe in its first column){tag}"
                )
            );
        }
        measured_state_checks(ctx, app, tag);
        return;
    };
    // PT: run.mjs:4770
    check!(
        ctx,
        l.name_pad >= 12.0 && l.name_pad < 80.0,
        format!("a row has its padding ({:.1}px){tag}", l.name_pad),
    );
    // The copy button's box: the pointer on the name, then on the button, and what changed.
    let (mid, band) = (row_mid(&l, 1), l.rows[1]);
    move_to(app, l.list.0 + 40.0, mid);
    settle_fade(ctx);
    let name_shot = ctx.shot(app, "help-hover-name").and_then(|p| Img::load(&p));
    move_to(app, copy_x(&l), mid);
    settle_fade(ctx);
    let btn_shot = ctx.shot(app, "help-hover-copy").and_then(|p| Img::load(&p));
    let dev = |css: f64| (css * k).round() as usize;
    let region = (
        dev(copy_x(&l) - 30.0),
        dev(band.0),
        dev(copy_x(&l) + 30.0),
        dev(band.1),
    );
    let bbox = match (&name_shot, &btn_shot) {
        (Some(a), Some(b)) => diff_bbox(a, b, region, 3),
        _ => None,
    };
    let (bw, bh, bright) = bbox.map_or((0.0, 0.0, 0.0), |(x0, y0, x1, y1)| {
        ((x1 - x0) as f64 / k, (y1 - y0) as f64 / k, x1 as f64 / k)
    });
    // PT: run.mjs:4771
    check!(
        ctx,
        bw >= 25.5 && bh >= 25.5,
        format!("the copy button is big enough to hit ({bw:.1}x{bh:.1}){tag}"),
    );
    // PT: run.mjs:4772
    check!(
        ctx,
        (bright - (l.list.0 + l.list.2 - 16.0)).abs() <= 4.0,
        format!(
            "and sits at the right edge of the row ({bright:.1} vs {:.1}){tag}",
            l.list.0 + l.list.2 - 16.0
        ),
    );
    park(app);
    // PT: run.mjs:4773
    let heights: Vec<i64> = (0..4).map(|i| l.height(i).round() as i64).collect();
    let mut distinct = heights.clone();
    distinct.sort_unstable();
    distinct.dedup();
    check!(
        ctx,
        distinct.len() <= 2 && heights.iter().all(|h| (34..=44).contains(h)),
        format!("a row is one or two lines, never taller ({heights:?}px){tag}"),
    );
    // PT: run.mjs:4774. The well starts at one x in every row; the names start where their glyphs do.
    let spread = l.well_x.iter().cloned().fold(f64::MIN, f64::max)
        - l.well_x.iter().cloned().fold(f64::MAX, f64::min);
    check!(
        ctx,
        l.well_x.len() == 3 && spread <= 1.5 / l.k,
        format!(
            "its two columns start at one x each (command well at {:?}, name ink at {:.1}){tag}",
            l.well_x, l.name_pad
        ),
    );
    // PT: run.mjs:4775. Square: the stripe's top corners are the stripe's own colour.
    let top = (l.rows[1].0 * l.k).round() as usize + 1;
    let (left, right) = (
        (l.list.0 * l.k).round() as usize,
        ((l.list.0 + l.list.2) * l.k).round() as usize - 1,
    );
    let square = l.shot.at(left, top) == l.stripe && l.shot.at(right, top) == l.stripe;
    check!(ctx, square, format!("square cornered{tag}"));
    // PT: run.mjs:4776. Row 0 plain, row 1 striped, row 2 plain, row 3 the same stripe.
    let at_row = |i: usize| {
        l.shot
            .at(left + 3, ((row_mid(&l, i)) * l.k).round() as usize)
    };
    let model: Vec<bool> = rows_json(&hs(app))
        .iter()
        .take(4)
        .map(|r| r["striped"] == json!(true))
        .collect();
    check!(
        ctx,
        at_row(0) == l.plain
            && at_row(1) != l.plain
            && at_row(2) == l.plain
            && at_row(3) == at_row(1)
            && model == [false, true, false, true],
        format!("the rows alternate in colour, the first row plain ({model:?}){tag}"),
    );
    measured_state_checks(ctx, app, tag);
}

/// The layout checks that read the panel's state: nothing marked, no sub text, the face, the scroll.
fn measured_state_checks(ctx: &mut Ctx, app: &mut App, tag: &str) {
    let st = hs(app);
    let rows = rows_json(&st);
    let marked = rows.iter().filter(|r| r["marked"] == json!(true)).count();
    // PT: run.mjs:4777
    check!(
        ctx,
        marked == 0,
        format!(
            "and NOTHING is marked until somebody points at it or walks the list ({marked} marked){tag}"
        ),
    );
    // PT: run.mjs:4778. A row is a name and a command; the model has no third text.
    let plain_rows = rows
        .iter()
        .filter(|r| {
            r["name"].as_str().unwrap_or("").is_empty()
                || r["command"].as_str().unwrap_or("").is_empty()
        })
        .count();
    check!(
        ctx,
        plain_rows == 0,
        format!(
            "no sub text: a row is a name and a command ({plain_rows} rows with another shape){tag}"
        ),
    );
    // PT: run.mjs:4779
    check!(
        ctx,
        st["mono"] == st["terminal_face"] && st["mono"].as_str().is_some_and(|m| !m.is_empty()),
        format!(
            "the command wears the terminal's face ({}){tag}",
            st["mono"]
        ),
    );
    // PT: run.mjs:4780. The list scrolls, the panel does not.
    let list = rect_of(&st["list"]).unwrap_or((0.0, 0.0, 600.0, 400.0));
    let before = rect_of(&st["panel"]);
    let y0 = st["viewport_y"].as_f64().unwrap_or(0.0);
    wheel(app, list.0 + list.2 / 2.0, list.1 + list.3 / 2.0, -200.0);
    let scrolled = ctx.until("the list to scroll", 3_000, &mut || {
        hs(app)["viewport_y"].as_f64().unwrap_or(0.0) < y0 - 1.0
    });
    let after = rect_of(&hs(app)["panel"]);
    check!(
        ctx,
        scrolled && before == after,
        format!(
            "the list scrolls, the panel does not ({y0} -> {}){tag}",
            hs(app)["viewport_y"]
        ),
    );
    wheel(app, list.0 + list.2 / 2.0, list.1 + list.3 / 2.0, 4000.0);
    ctx.until("the list to scroll back", 3_000, &mut || {
        hs(app)["viewport_y"].as_f64().unwrap_or(-1.0) >= -0.5
    });
    park(app);
    // PT: run.mjs:4784
    // DROPPED: "no sentence under the list" because an absent text has nothing to read at run time; the
    // Slint source is the record (e2e_probes/help.rs pins the footer's words to the three keys).
}

/// The edge draws nothing, the drag widens by 100, it is remembered, Right narrows, the default is
/// back. PT: run.mjs:4786-4833.
fn edge_and_width(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    let st = hs(app);
    let panel = rect_of(&st["panel"]).unwrap_or((0.0, 0.0, 0.0, 0.0));
    let wide0 = st["sheet_w"].as_f64().unwrap_or(0.0);
    let k = scale_of(app);
    // PT: run.mjs:4797. Inside the sheet the edge draws nothing of its own: a pixel 2 px in and one
    // 6 px in, in the head where nothing else is, are the same.
    let well_of =
        |l: &Look| l.list.0 + l.list.2 - 16.0 - l.well_x.first().copied().unwrap_or(f64::MAX);
    park(app);
    let was = measure(ctx, app, "help-narrow").map(|l| well_of(&l));
    let shot = ctx.shot(app, "help-edge").and_then(|p| Img::load(&p));
    let dev = |css: f64| (css * k).round() as usize;
    let quiet = shot.as_ref().is_some_and(|s| {
        let y = dev(BAR_H + 3.0);
        s.at(dev(panel.0 + 2.0), y) == s.at(dev(panel.0 + 6.0), y)
    });
    check!(ctx, quiet, "the edge draws nothing");
    let g = rect_of(&st["grip"]).unwrap_or((0.0, 0.0, 8.0, 100.0));
    let (gx, gy) = (g.0 + g.2 / 2.0, g.1 + g.3 / 2.0);
    drag(app, (gx, gy), (gx - 100.0, gy), 8);
    let widened = ctx.until("the panel to widen", 3_000, &mut || {
        (hs(app)["sheet_w"].as_f64().unwrap_or(0.0) - (wide0 + 100.0)).abs() <= 6.0
    });
    let wide1 = hs(app)["sheet_w"].as_f64().unwrap_or(0.0);
    // PT: run.mjs:4803
    check!(
        ctx,
        widened,
        format!("dragging the left edge 100px out widens it by 100 ({wide0} -> {wide1})")
    );
    // PT: run.mjs:4805. The command's well grew with it: measured from the snapshot.
    park(app);
    settle_slide(ctx);
    let grown = measure(ctx, app, "help-wide").map(|l| well_of(&l));
    check!(
        ctx,
        matches!((was, grown), (Some(w0), Some(w1)) if w1 > w0 + 20.0),
        format!("and the command column grew with it ({was:?} -> {grown:?})"),
    );
    // PT: run.mjs:4807. It is REMEMBERED: closed, opened again, and still that wide.
    let _ = app.session.key("Escape");
    closed(ctx, app);
    let _ = app.session.key("F1");
    opened(ctx, app);
    slid(ctx, app);
    let again = hs(app)["sheet_w"].as_f64().unwrap_or(0.0);
    check!(
        ctx,
        (again - wide1).abs() <= 2.0,
        format!("and it comes back that wide ({again})")
    );
    // PT: run.mjs:4812. The keyboard does it too, on the edge itself: a press on the edge focuses it
    // (no travel, no change), then Right narrows it by a step.
    settle_slide(ctx);
    let st = hs(app);
    let g = rect_of(&st["grip"]).unwrap_or((0.0, 0.0, 8.0, 100.0));
    click_at(app, g.0 + g.2 / 2.0, g.1 + g.3 / 2.0);
    let _ = app.session.key("ArrowRight");
    let narrowed = ctx.until("Right to narrow the panel", 3_000, &mut || {
        (hs(app)["sheet_w"].as_f64().unwrap_or(0.0) - (wide1 - 20.0)).abs() <= 2.0
    });
    check!(
        ctx,
        narrowed,
        format!(
            "Right on the edge narrows it a step ({})",
            hs(app)["sheet_w"]
        )
    );
    settle_slide(ctx);
    // PT: run.mjs:4820. PT clears the stored width; Flare has no clearing seam in the page, so the
    // edge is dragged back to the width it started with, and the width is what it was.
    let st = hs(app);
    let now = st["sheet_w"].as_f64().unwrap_or(0.0);
    let g = rect_of(&st["grip"]).unwrap_or((0.0, 0.0, 8.0, 100.0));
    let (gx, gy) = (g.0 + g.2 / 2.0, g.1 + g.3 / 2.0);
    drag(app, (gx, gy), (gx + (now - wide0), gy), 8);
    let back = ctx.until("the width to come back", 3_000, &mut || {
        (hs(app)["sheet_w"].as_f64().unwrap_or(0.0) - wide0).abs() <= 2.0
    });
    let _ = app.session.key("Escape");
    closed(ctx, app);
    let _ = app.session.key("F1");
    opened(ctx, app);
    slid(ctx, app);
    check!(
        ctx,
        back && (hs(app)["sheet_w"].as_f64().unwrap_or(0.0) - wide0).abs() <= 2.0,
        format!("and it is the default width again ({})", hs(app)["sheet_w"]),
    );
    // Put the question back: everything below measures the answers to this one.
    set_query(app, BIG_FILES);
    asked(ctx, app, "ps-biggest-files");
    Ok(())
}

/// Copying: the check, the badge, the exact text, the whole row, the well, the selection.
/// PT: run.mjs:4835-4908.
fn copy_checks(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    park(app);
    let Some(l) = measure(ctx, app, "help-copy-before") else {
        check!(ctx, false, "the list could not be measured before the copy");
        return Ok(());
    };
    let st = hs(app);
    let rows = rows_json(&st);
    let want_main = rows
        .first()
        .and_then(|r| r["command"].as_str())
        .unwrap_or("")
        .to_string();
    let height_before = l.height(0);
    click_copy(app, &l, 0);
    // PT: run.mjs:4850. The icon turns into a check on the row, and the app's Copied badge stays down.
    let checked = ctx.until("the check", 3_000, &mut || {
        rows_json(&hs(app))
            .first()
            .is_some_and(|r| r["copied"] == json!(true))
    });
    check!(ctx, checked, "a copy turns its icon into a check");
    // PT: run.mjs:4851
    let badge = probe(app, "app.state", Value::Null)["badge"]["visible"] == json!(true);
    check!(ctx, !badge, "and raises no \"Copied\" badge");
    // PT: run.mjs:4852. The words a screen reader hears are the panel's own `said` text.
    let said = hs(app)["said"].as_str().unwrap_or("").to_string();
    check!(
        ctx,
        said.contains("Copied"),
        format!("a screen reader is told, by the panel (\"{said}\")")
    );
    ctx.shot(app, "help-copied-dark");
    park(app);
    let during = measure(ctx, app, "help-copy-during").map(|m| m.height(0));
    // PT: run.mjs:4856
    check!(
        ctx,
        during.is_some_and(|h| (h - height_before).abs() < 0.6),
        format!("and the row does not change height ({height_before:.1} -> {during:?})"),
    );
    // PT: run.mjs:4857
    let held = clip(app);
    check!(
        ctx,
        !want_main.is_empty() && want_main.contains("Sort-Object") && held == want_main,
        format!("the clipboard holds the EXACT command (\"{held}\")"),
    );
    // PT: run.mjs:4858
    check!(
        ctx,
        ctx.until("the check to leave by itself", 4_000, &mut || {
            !rows_json(&hs(app))
                .first()
                .is_some_and(|r| r["copied"] == json!(true))
        }),
        "the answer leaves by itself",
    );
    park(app);
    let after = measure(ctx, app, "help-copy-after").map(|m| m.height(0));
    // PT: run.mjs:4865
    check!(
        ctx,
        after.is_some_and(|h| (h - height_before).abs() < 0.6),
        "still without moving anything",
    );
    Ok(())
}

/// Part B: the rest of the copy checks, the keyboard, a warning, the terminal, tabs, find, the menu,
/// a light theme, off means off, one question at a time. PT: run.mjs:4866-5072.
fn help_b(ctx: &mut Ctx, app: &mut App, started: Instant) -> Result<(), String> {
    let term_before =
        std::fs::read_to_string(app.profile.join("help-term-before.txt")).unwrap_or_default();
    park(app);
    let Some(l) = measure(ctx, app, "help-copy-rows") else {
        check!(
            ctx,
            false,
            "the list could not be measured for the row copies"
        );
        return Ok(());
    };
    let st = hs(app);
    let rows = rows_json(&st);
    let want_main = rows
        .first()
        .and_then(|r| r["command"].as_str())
        .unwrap_or("")
        .to_string();
    let variant = rows.get(1).cloned().unwrap_or(Value::Null);
    let want_variant = variant["command"].as_str().unwrap_or("").to_string();
    // PT: run.mjs:4870. A variant is a row of its own, named by what was its label.
    check!(
        ctx,
        variant["id"] == json!("ps-biggest-files")
            && variant["variant"] == json!(1)
            && variant["name"].as_str().unwrap_or("").trim().len() > 3,
        format!(
            "a variant is a row of its own, with a name (\"{}\")",
            variant["name"].as_str().unwrap_or("")
        ),
    );
    // PT: run.mjs:4875. THE WHOLE ROW COPIES, not only its button.
    click_name(app, &l, 1);
    let row_copied = ctx.until("a click on a row to copy it", 3_000, &mut || {
        clip(app) == want_variant
    });
    check!(
        ctx,
        !want_variant.is_empty() && want_variant != want_main && row_copied,
        "a click on a row copies that row",
    );
    // PT: run.mjs:4881. BUT NOT A CLICK IN ITS COMMAND'S WELL: there only the copy button copies.
    park(app);
    let well_x = l.well_x.first().copied().unwrap_or(l.list.0 + 200.0);
    let main_mid = (l.rows[0].0 + l.rows[0].1) / 2.0;
    // PT clicks the command itself, then the corner of its box.
    click_at(app, well_x + 40.0, main_mid);
    click_at(app, well_x + 3.0, main_mid);
    ctx.settle(50);
    let held = clip(app);
    check!(
        ctx,
        held == want_variant,
        format!("a click in the command's box copies nothing ({held})"),
    );
    // PT: run.mjs:4886-4889. The text cursor over a command is a Slint property (the source scan in
    // e2e_probes/help.rs pins it); that the command is text to SELECT is what a drag in it shows.
    // DROPPED: "the text cursor over a command" because the cursor shape cannot be read from a window
    // that never shows; the same file pins `mouse-cursor: MouseCursor.text` on the well.
    park(app);
    // macOS: the row's hover fades out over 120 ms, so the picture before is taken once two shots in a row
    // agree, or a row still lit by the clicks above is the "changed colour" (the Mac runner, rule 1119).
    // Windows takes it at once, as before.
    let plain_shot = if cfg!(target_os = "macos") {
        still_shot(ctx, app, "help-select-before")
    } else {
        ctx.shot(app, "help-select-before")
            .and_then(|p| Img::load(&p))
    };
    let k = l.k;
    let dev = |css: f64| (css * k).round() as usize;
    let row0 = l.rows[0];
    let (cx0, cx1) = (well_x + 14.0, well_x + 90.0);
    drag(app, (cx0, main_mid), (cx1, main_mid), 6);
    ctx.settle(50);
    let sel_shot = ctx
        .shot(app, "help-select-after")
        .and_then(|p| Img::load(&p));
    let region = (dev(well_x), dev(row0.0), dev(well_x + 140.0), dev(row0.1));
    let accent = rgb_of_hex(hs(app)["ink"]["accent"].as_str().unwrap_or("#000000ff"));
    let (changed, mode) = match (&plain_shot, &sel_shot) {
        (Some(a), Some(b)) => (
            diff_bbox(a, b, region, 6).is_some(),
            changed_mode(a, b, region, 6),
        ),
        _ => (false, None),
    };
    // PT: run.mjs:4892. Its selection wears the accent: the commonest changed colour is the well
    // with the accent laid over it, not a default blue.
    let well_px = plain_shot
        .as_ref()
        .map(|s| s.at(dev(well_x + 30.0), dev(main_mid)));
    let wears = match (mode, well_px) {
        (Some(m), Some(w)) => {
            let want = crate::pixels::mix(w, accent, 0.45);
            (0..3).all(|i| m[i].abs_diff(want[i]) <= 14)
                && !(m[0] < 20 && m[1].abs_diff(120) < 20 && m[2].abs_diff(215) < 20)
        }
        _ => false,
    };
    check!(
        ctx,
        wears,
        format!("its selection wears the accent ({mode:?} over {well_px:?}, accent {accent:?})")
    );
    // PT: run.mjs:4893
    check!(ctx, changed, "the command can be selected");
    // PT: run.mjs:4895. A DRAG copies nothing: from inside the command out onto the row's name.
    let before_drag = clip(app);
    drag(
        app,
        (well_x + 150.0, main_mid),
        (l.list.0 + 40.0, main_mid),
        8,
    );
    ctx.settle(50);
    let held = clip(app);
    check!(
        ctx,
        held == before_drag && held == want_variant,
        format!("a drag from the command onto the row copies nothing ({held})"),
    );
    // PT: run.mjs:4911. A SELECTION LEFT OVER blocks nothing: a click on the row's name still copies.
    drag(app, (well_x + 14.0, main_mid), (well_x + 60.0, main_mid), 4);
    click_name(app, &l, 0);
    let again = ctx.until(
        "a click on the name to copy the main row",
        3_000,
        &mut || clip(app) == want_main,
    );
    check!(
        ctx,
        again,
        format!(
            "with a command still selected, a click on a row's name copies ({})",
            clip(app)
        )
    );

    /* ----- the keyboard ----- */
    set_query(app, BIG_FILES);
    asked(ctx, app, "ps-biggest-files");
    let from = marked_at(app);
    let _ = app.session.key("ArrowDown");
    let to = {
        let mut got = None;
        ctx.until("a row to be marked", 3_000, &mut || {
            got = marked_at(app).filter(|n| Some(*n) != from);
            got.is_some()
        });
        got
    };
    // PT: run.mjs:4929. With nothing marked the first Down marks where the cursor is: the top, or
    // the row last clicked (a click copies it and puts the cursor there).
    let moved = match (from, to) {
        (Some(f), Some(t)) => t == f + 1,
        (None, Some(t)) => t < rows_json(&hs(app)).len(),
        _ => false,
    };
    check!(
        ctx,
        moved,
        format!("Down marks a row, then moves it ({from:?} -> {to:?})")
    );
    let want_second = to
        .and_then(|t| {
            rows_json(&hs(app))
                .get(t)
                .and_then(|r| r["command"].as_str().map(str::to_string))
        })
        .unwrap_or_default();
    let _ = app.session.key("Enter");
    // PT: run.mjs:4932
    let entered = ctx.until("Enter to copy the marked row", 3_000, &mut || {
        clip(app) == want_second
    });
    check!(
        ctx,
        !want_second.is_empty() && entered,
        "Enter copies the marked row's command"
    );
    let _ = app.session.key("ArrowUp");
    // PT: run.mjs:4936. The pointer moving over the list takes the keyboard's mark away.
    let list = rect_of(&hs(app)["list"]).unwrap_or((0.0, 0.0, 600.0, 400.0));
    move_to(app, list.0 + 50.0, list.1 + 60.0);
    move_to(app, list.0 + 60.0, list.1 + 80.0);
    check!(
        ctx,
        ctx.until("the pointer to take the mark away", 2_000, &mut || {
            marked_at(app).is_none()
        }),
        "the pointer over the list takes the mark away",
    );

    /* ----- a warning, and a key that is not a command ----- */
    set_query(app, "delete a folder");
    // PT: run.mjs:4945
    let found = asked(ctx, app, "ps-delete-folder");
    check!(
        ctx,
        found,
        format!("\"delete a folder\" finds it ({})", first_id(app))
    );
    park(app);
    // PT: run.mjs:4949. NO WARNING LABELS: no amber is drawn in the row.
    let dl = measure(ctx, app, "help-danger");
    let amber = dl.as_ref().is_some_and(|d| {
        let (r0, r1) = (d.rows[0], d.rows[1]);
        let k = d.k;
        let mut hit = false;
        for y in (r0.0 * k) as usize..(r1.0 * k) as usize {
            for x in (d.list.0 * k) as usize..((d.list.0 + d.list.2) * k) as usize {
                let p = d.shot.at(x, y);
                // #e0a100, the page's warning amber
                if p[0].abs_diff(0xe0) < 24 && p[1].abs_diff(0xa1) < 24 && p[2] < 40 {
                    hit = true;
                }
            }
        }
        hit
    });
    let danger_row = rows_json(&hs(app))
        .into_iter()
        .find(|r| r["id"] == json!("ps-delete-folder") && r["variant"] == json!(0));
    check!(ctx, dl.is_some() && !amber, "no warning mark is drawn");
    // PT: run.mjs:4951. A screen reader is still told: the row carries `Careful.` in its accessible
    // description (the source scan in e2e_probes/help.rs pins the words), and the model says danger.
    check!(
        ctx,
        danger_row
            .as_ref()
            .is_some_and(|r| r["danger"] == json!(true)),
        "and a screen reader is still told (the row is flagged careful)",
    );
    ctx.shot(app, "help-danger-dark");
    set_query(app, "stop a running command");
    // PT: run.mjs:4962
    let keys = ctx.until("the key row", 4_000, &mut || {
        rows_json(&hs(app))
            .iter()
            .any(|r| r["id"] == json!("ps-cancel-command"))
    });
    let key_row = rows_json(&hs(app))
        .into_iter()
        .find(|r| r["id"] == json!("ps-cancel-command") && r["variant"] == json!(0));
    check!(
        ctx,
        keys && key_row
            .as_ref()
            .is_some_and(|r| r["caps"].as_u64().unwrap_or(0) >= 2 && r["keys"] == json!(true)),
        format!(
            "a key to press is drawn as keys and has no copy button ({:?})",
            key_row.as_ref().map(|r| r["caps"].clone())
        ),
    );
    set_query(app, "zzzz qqqq xxxx");
    // PT: run.mjs:4965
    check!(
        ctx,
        ctx.until("the empty answer", 3_000, &mut || hs(app)["empty"]
            == json!(true)),
        "a question nothing answers says so",
    );

    /* ----- nothing was typed into the shell ----- */
    // PT: run.mjs:4968
    let now = term_text(app);
    check!(
        ctx,
        now == term_before,
        "the terminal is EXACTLY as it was: nothing was typed or run"
    );
    let _ = app.session.key("Escape");
    // PT: run.mjs:4970
    check!(ctx, closed(ctx, app), "Escape closes it");
    // No click: the keyboard has to be back in the shell by itself.
    let _ = app.session.type_text("echo landed-$(1+1)");
    let _ = app.session.key("Enter");
    // PT: run.mjs:4974
    check!(
        ctx,
        ctx.until("the next keystroke to land", 15_000, &mut || term_text(app)
            .contains("landed-2")),
        format!(
            "and the next keystroke lands in the shell (field focused: {})",
            hs(app)["field_focused"]
        ),
    );

    /* ----- a new tab: the panel stays, the shell gets the keyboard ----- */
    let _ = app.session.key("F1");
    // PT: run.mjs:4981
    check!(ctx, opened(ctx, app), "the panel is out again");
    // The field takes the keys 60 ms into the slide: a tab opened before that would be undone by it.
    slid(ctx, app);
    let _ = app.session.key("Ctrl+T");
    // PT: run.mjs:4983
    check!(
        ctx,
        ctx.until("a second tab", 15_000, &mut || tab_count(app) == 2),
        "Ctrl+T opens a tab"
    );
    // PT: run.mjs:4984
    check!(ctx, is_open(app), "and the panel stays out beside it");
    // PT: run.mjs:4985. The new shell has the keyboard: a typed line reaches it.
    let front = front_id(app);
    let prompt = wait_for_prompt(
        app,
        &front,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    );
    let field_had_it = field_focused(app);
    if prompt {
        // A fresh PowerShell takes a burst of typed characters for a paste and keeps the Enter as a
        // line break: the same mute line the first shell gets quiets it here too.
        mute_psreadline_paste(app);
        wait_for_prompt(
            app,
            &front,
            Shell::PowerShell,
            prompt_timeout(Shell::PowerShell),
        );
        type_line(app, "echo kb-$(1+1)");
    }
    let landed = prompt
        && ctx.until("kb-2 in the new shell", 15_000, &mut || {
            term_text(app).contains("kb-2")
        });
    let shown: String = {
        let t = term_text(app);
        let tail: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty()).collect();
        let last = tail.last().copied().unwrap_or("");
        // The code points too: the Mac runner showed bytes no key sends (^E, ^O, invalid UTF-8).
        let points: Vec<String> = last
            .chars()
            .rev()
            .take(48)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|c| format!("{:x}", u32::from(c)))
            .collect();
        format!(
            "{} [last line's code points: {}]",
            tail[tail.len().saturating_sub(4)..].join(" | "),
            points.join(" ")
        )
    };
    let written = if landed {
        String::new()
    } else {
        let _ = ctx.shot(app, "help-new-shell");
        let g = probe(app, "grid.text", Value::Null);
        format!(
            "; its grid {} cols in a box {:?} css px wide, {} OS frames; Flare wrote to it: {}",
            g["cols"],
            rect_of(&probe(app, "ui.rect", json!({ "name": "grid" }))).map(|r| r.2),
            probe(app, "sidebar.state", Value::Null)["redraws"],
            probe(app, "pty.written", Value::Null)
        )
    };
    check!(
        ctx,
        landed,
        format!(
            "the new shell has the keyboard (the panel's field held it: {field_had_it}, its prompt came: {prompt}, \
             field now: {}, the shell shows: {shown}{written})",
            hs(app)["field_focused"]
        )
    );
    let _ = app.session.key("Ctrl+W");
    // PT: run.mjs:4987
    check!(
        ctx,
        ctx.until("the extra tab to close", 8_000, &mut || tab_count(app) == 1),
        "the extra tab closes again"
    );
    // PT: run.mjs:4988
    check!(ctx, is_open(app), "and the panel is still out");
    let _ = app.session.key("Ctrl+Shift+F");
    // PT: run.mjs:4990. Flare stacks them (the one opened last is on top): find is in front.
    let find = ctx.until("the find bar", 4_000, &mut || {
        hs(app)["find_open"] == json!(true)
    });
    check!(ctx, find, "Ctrl+Shift+F opens find");
    // PT: run.mjs:4991
    let st = hs(app);
    check!(
        ctx,
        st["on_top"] == json!(false) && st["find_open"] == json!(true),
        "and the panel goes in, since find sits where it is (find is on top)",
    );
    let _ = app.session.key("Escape");
    // PT: run.mjs:4993
    check!(
        ctx,
        ctx.until("find to close", 4_000, &mut || hs(app)["find_open"]
            == json!(false)),
        "Escape closes find",
    );
    // The panel may still be out beside find; put it away so the menu check starts clean.
    if is_open(app) {
        let r = rect_of(&hs(app)["close"]);
        if let Some(r) = r {
            click_rect(app, r);
        }
        closed(ctx, app);
    }

    /* ----- the terminal's own menu ----- */
    let g = grid_rect(app);
    let _ = app
        .session
        .right_click((g.0 + 200.0) as f32, (g.1 + 120.0) as f32);
    let menu_has = ctx.until("a menu row Command help", 4_000, &mut || {
        menu_labels(app).iter().any(|l| l == "Command help")
    });
    // PT: run.mjs:4998
    check!(ctx, menu_has, "the right-click menu has a Command help row");
    // Command help is the menu's last row: the click lands on the bottom row of the menu's box.
    if let Some(m) = rect_of(&probe(app, "ui.rect", json!({ "name": "menu" }))) {
        click_at(app, m.0 + m.2 / 2.0, m.1 + m.3 - 18.0);
    }
    // PT: run.mjs:5000
    check!(ctx, opened(ctx, app), "which opens it");
    // The X is where the slide ends: wait for it, or the press lands on a moving button.
    slid(ctx, app);
    if let Some(r) = rect_of(&hs(app)["close"]) {
        click_rect(app, r);
    }
    // PT: run.mjs:5002
    check!(ctx, closed(ctx, app), "and its own X closes it");

    /* ----- a light theme, looked at ----- */
    light_theme_checks(ctx, app)?;

    /* ----- off means off ----- */
    switch_checks(ctx, app)?;

    /* ----- one question at a time ----- */
    question_checks(ctx, app, started)
}

/// The labels of the open menu's rows.
fn menu_labels(app: &mut App) -> Vec<String> {
    probe(app, "app.state", Value::Null)["menu"]["rows"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| r["label"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn settings_front(app: &mut App) -> bool {
    probe(app, "settings.state", Value::Null)["front"] == json!(true)
}

/// Opens Settings from the title bar's cog and waits until it is in front.
fn open_settings(ctx: &mut Ctx, app: &mut App) -> bool {
    if settings_front(app) {
        return true;
    }
    if let Some(r) = rect_of(&probe(app, "ui.rect", json!({ "name": "cog" }))) {
        click_rect(app, r);
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
    if let Some(r) = rect_of(&r) {
        click_rect(app, r);
    }
    ctx.until(&format!("the {id} page is drawn"), 6_000, &mut || {
        probe(app, "settings.rows", json!({}))["page"].as_str() == Some(id)
    })
}

/// PT: run.mjs:5005-5030. A paper theme, and the panel over Settings, read in light.
fn light_theme_checks(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    let at_settings = open_settings(ctx, app) && goto_page(ctx, app, "appearance");
    // The paper card when the wall shows it, else any light card; the wall is wheeled until it is in view.
    let g = grid_rect(app);
    let picked = at_settings
        && ctx.until("a light theme card in view", 12_000, &mut || {
            let wall = probe(app, "picker.wall", Value::Null);
            let cards = wall["cards"].as_array().cloned().unwrap_or_default();
            let light = |c: &Value| lum(rgb_of_hex(c["bg"].as_str().unwrap_or("#000000ff"))) > 0.4;
            let pick = cards
                .iter()
                .find(|c| c["id"] == json!("paper") && c["shown"] == json!(true))
                .or_else(|| cards.iter().find(|c| light(c) && c["shown"] == json!(true)))
                .cloned();
            match pick {
                Some(c) if c["visible"] == json!(true) => {
                    if let Some(r) = rect_of(&c["rect"]) {
                        click_rect(app, r);
                    }
                    true
                }
                Some(c) => {
                    let y = c["rect"]["y"].as_f64().unwrap_or(0.0);
                    let dy = if y > g.1 + g.3 / 2.0 { -160.0 } else { 160.0 };
                    wheel(app, g.0 + g.2 / 2.0, g.1 + g.3 / 2.0, dy);
                    false
                }
                None if wall["expanded"] == json!(false) => {
                    // Only the first themes show until the wall is opened out.
                    if wall["toggle_visible"] == json!(true) {
                        if let Some(r) = rect_of(&wall["toggle"]) {
                            click_rect(app, r);
                        }
                    } else {
                        let y = wall["toggle"]["y"].as_f64().unwrap_or(0.0);
                        let dy = if y > g.1 + g.3 / 2.0 { -160.0 } else { 160.0 };
                        wheel(app, g.0 + g.2 / 2.0, g.1 + g.3 / 2.0, dy);
                    }
                    false
                }
                None => false,
            }
        });
    let light = picked
        && ctx.until("a light ground", 6_000, &mut || {
            probe(app, "window.ground", Value::Null)["light"] == json!(true)
        });
    let _ = app.session.key("F1");
    // PT: run.mjs:5014
    check!(ctx, opened(ctx, app), "F1 opens it over Settings too");
    slid(ctx, app);
    ctx.shot(app, "help-browse-light");
    let _ = app.session.type_text("delete a folder");
    let found = asked(ctx, app, "ps-delete-folder");
    // The row's copy button: row 0 of the answers.
    park(app);
    if let Some(l) = measure(ctx, app, "help-light") {
        click_copy(app, &l, 0);
    }
    ctx.until("the check", 3_000, &mut || {
        rows_json(&hs(app))
            .first()
            .is_some_and(|r| r["copied"] == json!(true))
    });
    ctx.shot(app, "help-results-light");
    // PT: run.mjs:5038. The command's ink against the panel's own ground, as the page read them.
    let st = hs(app);
    let ink = rgb_of_hex(st["ink"]["text"].as_str().unwrap_or("#000000ff"));
    let ground = rgb_of_hex(st["ink"]["ground"].as_str().unwrap_or("#ffffffff"));
    let ratio = contrast(ink, ground);
    check!(
        ctx,
        light && found && ratio >= 4.5 && lum(ground) > lum(ink),
        format!("on a light theme the command still reads ({ratio:.1}:1)"),
    );
    let _ = app.session.key("Escape");
    closed(ctx, app);
    Ok(())
}

/// Finds the Command help switch on the Terminal page and flips it with a click on its right end.
fn flip_help_switch(ctx: &mut Ctx, app: &mut App) -> Option<bool> {
    let cat = probe(app, "settings.rows", json!({ "catalogue": true }));
    let page = cat["rows"]
        .as_array()?
        .iter()
        .find(|r| r["id"] == json!("help-enabled"))
        .and_then(|r| r["page"].as_str().map(str::to_string))?;
    if !open_settings(ctx, app) || !goto_page(ctx, app, &page) {
        return None;
    }
    let reveal = ctx.until("the Command help row in view", 12_000, &mut || {
        let r = probe(app, "settings.row", json!({ "id": "help-enabled" }));
        if r["visible"] == json!(true) {
            return true;
        }
        let g = grid_rect(app);
        let y = r["rect"]["y"].as_f64().unwrap_or(0.0);
        let dy = if y > g.1 + g.3 / 2.0 { -160.0 } else { 160.0 };
        wheel(app, g.0 + g.2 / 2.0, g.1 + g.3 / 2.0, dy);
        false
    });
    if !reveal {
        return None;
    }
    let row = probe(app, "settings.row", json!({ "id": "help-enabled" }));
    let was = row["on"].as_bool()?;
    let r = rect_of(&row["rect"])?;
    for inset in [40.0, 28.0, 52.0, 64.0] {
        click_at(app, r.0 + r.2 - inset, r.1 + r.3 / 2.0);
        let now = probe(app, "settings.row", json!({ "id": "help-enabled" }))["on"].as_bool();
        if now == Some(!was) {
            return Some(!was);
        }
    }
    Some(was)
}

/// PT: run.mjs:5033-5057. The switch is on by default; off, the button, F1 and the menu row go.
fn switch_checks(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    let row_on = flip_help_switch_state(ctx, app);
    // PT: run.mjs:5036
    check!(
        ctx,
        row_on == Some(true),
        "Settings > Terminal has the Command help switch, on by default"
    );
    let flipped = flip_help_switch(ctx, app);
    // PT: run.mjs:5038
    check!(
        ctx,
        flipped == Some(false)
            && ctx.until("the ? to leave the title bar", 4_000, &mut || hs(app)
                ["button"]
                == json!(false)),
        "switched off, the ? leaves the title bar",
    );
    let _ = app.session.key("Ctrl+1");
    click_terminal(app);
    let _ = app.session.key("F1");
    // PT: run.mjs:5042
    check!(
        ctx,
        never(ctx, app, 1_500, &mut |a| is_open(a)),
        "and F1 opens nothing"
    );
    let g = grid_rect(app);
    let _ = app
        .session
        .right_click((g.0 + 200.0) as f32, (g.1 + 120.0) as f32);
    ctx.until("a menu", 4_000, &mut || !menu_labels(app).is_empty());
    // PT: run.mjs:5046
    check!(
        ctx,
        !menu_labels(app).iter().any(|l| l == "Command help"),
        "nor does the menu offer it"
    );
    let _ = app.session.key("Escape");
    let back = flip_help_switch(ctx, app);
    // PT: run.mjs:5051
    check!(
        ctx,
        back == Some(true)
            && ctx.until("the ? to return", 4_000, &mut || hs(app)["button"]
                == json!(true)),
        "switched back on, it returns",
    );
    let _ = app.session.key("Ctrl+1");
    Ok(())
}

/// The switch's state without changing it: the row, once the page shows it.
fn flip_help_switch_state(ctx: &mut Ctx, app: &mut App) -> Option<bool> {
    let cat = probe(app, "settings.rows", json!({ "catalogue": true }));
    let page = cat["rows"]
        .as_array()?
        .iter()
        .find(|r| r["id"] == json!("help-enabled"))
        .and_then(|r| r["page"].as_str().map(str::to_string))?;
    if !open_settings(ctx, app) || !goto_page(ctx, app, &page) {
        return None;
    }
    probe(app, "settings.row", json!({ "id": "help-enabled" }))["on"].as_bool()
}

/// PT: run.mjs:5059-5072. One question at a time: a close question is never underneath the panel.
fn question_checks(ctx: &mut Ctx, app: &mut App, started: Instant) -> Result<(), String> {
    // A shell stands in for Claude through the title. The process poll's verdict on a shell is said
    // once and clears a title's claim when it lands; by now this shell has been printing for longer
    // than the poll's first look takes, and the wait only makes that certain on a fast run.
    ctx.until("the poll's first look to be over", 15_000, &mut || {
        started.elapsed() > Duration::from_secs(12)
    });
    click_terminal(app);
    // A real agent process in the tab (review #47): a title is a claim, and one look takes it back when
    // no agent runs there.
    if let Ok(cmd) = crate::standins::agent_proc::command(&app.profile) {
        type_line(app, &cmd);
        let start = std::time::Instant::now();
        ctx.until(
            "the poll to have seen the stand-in agents",
            10_000,
            &mut || {
                start.elapsed()
                    >= std::time::Duration::from_millis(crate::standins::agent_proc::SETTLE_MS)
            },
        );
    }
    type_line(
        app,
        "$Host.UI.RawUI.WindowTitle = [char]0x2733 + ' Claude Code'",
    );
    // PT: run.mjs:5068
    let present = ctx.until("an agent in the tab", 8_000, &mut || {
        probe(app, "app.state", Value::Null)["tabs"]
            .as_array()
            .is_some_and(|t| t.iter().any(|t| t["kind"] == json!("claude")))
    });
    check!(
        ctx,
        present,
        "an agent is present in the tab, so closing it would ask"
    );
    let _ = app.session.key("F1");
    // PT: run.mjs:5072
    check!(ctx, opened(ctx, app), "the popup is up");
    let _ = app.session.key("Ctrl+W");
    let dialog =
        |app: &mut App| probe(app, "app.state", Value::Null)["dialog"]["open"] == json!(true);
    // PT: run.mjs:5075
    check!(
        ctx,
        ctx.until("the close question", 4_000, &mut || dialog(app)),
        "Ctrl+W over it raises the close question"
    );
    // PT: run.mjs:5076
    check!(
        ctx,
        closed(ctx, app),
        "and the popup is put away, so the question is never underneath it"
    );
    // PT: run.mjs:5077. The panel's field does not hold the keys, so they are the question's.
    check!(
        ctx,
        dialog(app) && !field_focused(app),
        "the focus is on the question, where it can be seen",
    );
    let _ = app.session.key("F1");
    // PT: run.mjs:5082
    check!(
        ctx,
        never(ctx, app, 1_200, &mut |a| is_open(a)),
        "and F1 does not open it over a question"
    );
    let _ = app.session.key("Escape");
    // PT: run.mjs:5084
    check!(
        ctx,
        ctx.until("the question to close", 4_000, &mut || !dialog(app)),
        "Cancel keeps the tab"
    );
    // PT: run.mjs:5085
    let alpha = ctx.ensure_world().alpha.clone();
    let name = alpha
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let tabs = app.session.state().map(|s| s.tabs).unwrap_or_default();
    check!(ctx,
        tabs.iter().any(|t| t.title.to_lowercase().contains(&name) || t.cwd.to_lowercase().contains(&name)),
        "which is still there",
    );
    Ok(())
}

/// The layout again at 225 percent, in a fresh app. PT: the same measurements as run.mjs:4723-4783.
fn second_pass_225(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let mut app = ctx.launch(Launch {
        folders: vec![alpha],
        renderer: Some("software"),
        scale_dpi: Some(DPI_225),
        ..Launch::default()
    })?;
    let _ = app.session.call(&Request::Focus { on: true });
    if ctx.until("the one tab", 15_000, &mut || tab_count(&mut app) == 1) {
        // The window keeps its logical size through a scale change and grows; a window as tall as the
        // list would never scroll, so it goes back to the size of the first pass.
        let _ = app.session.call(&Request::Resize {
            w: 1180.0,
            h: 780.0,
        });
        let front = front_id(&mut app);
        wait_for_prompt(
            &mut app,
            &front,
            Shell::PowerShell,
            prompt_timeout(Shell::PowerShell),
        );
        if let Some(r) = rect_of(&probe(&mut app, "ui.rect", json!({ "name": "help" }))) {
            click_rect(&mut app, r);
        }
        if opened(ctx, &mut app) && slid(ctx, &mut app) {
            set_query(&mut app, BIG_FILES);
            asked(ctx, &mut app, "ps-biggest-files");
            layout_checks(ctx, &mut app, " [225%]");
        }
    }
    app.close();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_split_a_column_into_plain_and_other() {
        let p = [10, 10, 10];
        let s = [20, 20, 20];
        let col = [p, p, p, s, s, p, p, s];
        assert_eq!(
            runs(&col, p),
            vec![(true, 0, 3), (false, 3, 5), (true, 5, 7), (false, 7, 8)]
        );
        assert_eq!(runs(&[], p), vec![]);
    }

    #[test]
    fn a_diff_box_holds_exactly_the_changed_pixels() {
        let a = Img::solid(20, 10, [5, 5, 5]);
        let mut b = Img::solid(20, 10, [5, 5, 5]);
        assert_eq!(diff_bbox(&a, &b, (0, 0, 20, 10), 3), None);
        b.put(4, 2, [60, 5, 5]);
        b.put(8, 6, [5, 60, 5]);
        assert_eq!(diff_bbox(&a, &b, (0, 0, 20, 10), 3), Some((4, 2, 9, 7)));
        // The region limits the look, and a change under the threshold is no change.
        assert_eq!(diff_bbox(&a, &b, (0, 0, 6, 10), 3), Some((4, 2, 5, 3)));
        b.put(15, 9, [6, 5, 5]);
        assert_eq!(diff_bbox(&a, &b, (10, 0, 20, 10), 3), None);
    }

    #[test]
    fn the_commonest_changed_colour_is_the_selection_not_the_glyph() {
        let a = Img::solid(10, 4, [5, 5, 5]);
        let mut b = Img::solid(10, 4, [5, 5, 5]);
        for x in 0..8 {
            b.put(x, 1, [40, 60, 90]);
        }
        b.put(3, 2, [250, 250, 250]);
        assert_eq!(changed_mode(&a, &b, (0, 0, 10, 4), 6), Some([40, 60, 90]));
        assert_eq!(changed_mode(&a, &a, (0, 0, 10, 4), 6), None);
    }

    #[test]
    fn rgb_of_hex_reads_the_first_three_bytes() {
        assert_eq!(rgb_of_hex("#1f1f1eff"), [0x1f, 0x1f, 0x1e]);
        assert_eq!(rgb_of_hex("zz"), [0, 0, 0]);
    }

    #[test]
    fn a_decoded_png_matches_the_pixels_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.png");
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), 2, 1);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[1, 2, 3, 255, 9, 8, 7, 255]).unwrap();
        }
        let img = Img::load(&path).unwrap();
        assert_eq!((img.w, img.h), (2, 1));
        assert_eq!(img.at(0, 0), [1, 2, 3]);
        assert_eq!(img.at(1, 0), [9, 8, 7]);
        assert_eq!(img.at(5, 5), [0, 0, 0]);
    }

    #[test]
    fn every_ok_of_the_original_has_a_check_here() {
        // Appendix B: helpPanel has 89 `ok(` calls. The 225 percent pass reuses `layout_checks`, so
        // the source count is the original's plus nothing; the two DROPPED notes stand in for the
        // two checks that have no run-time reading.
        let src = include_str!("help.rs");
        let body = src.split("\nmod tests {").next().unwrap();
        // One of the matches is the macro's own doc line.
        let oks = body.matches("check!(").count() - 1;
        let dropped = body.matches("// DROPPED:").count();
        assert!(dropped <= 3, "{dropped} dropped");
        assert!(
            (88..=96).contains(&(oks + dropped)),
            "{oks} checks and {dropped} dropped"
        );
    }
}
