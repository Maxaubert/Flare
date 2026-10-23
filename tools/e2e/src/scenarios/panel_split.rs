//! The panel splitter, dragged by a hand (Surfaces, 2026-10-22): `panelSplitter`.
//!
//! Owner, 2026-10-22: "when moving the sidepanel to grow it or shrink it will bug out". This drives the built exe
//! (GPU renderer, offscreen, its own profile) with real pointer events: press on the splitter, many moves both ways,
//! quick and slow, past both limits, and a release, over a picture and over a text file, at 100 and 225 percent. At
//! every step it reads `panel.state`; at some it takes a picture of the window and reads pixels. What must always hold:
//!
//! - the slot (as drawn) and the splitter stay inside the limits, the splitter sits on the slot's left edge;
//! - the slot follows the pointer to the device pixel, and never moves against it;
//! - the grid keeps its width and is not fitted while the button is down (rule 24); it is fitted exactly once on the
//!   release, and then ends where the slot starts (no gap, no overlap);
//! - the panel's content is on every frame, during the drag and after the release (no blank frame);
//! - what shows beside the slot when it is narrower than it began is the terminal's ground, not the page's;
//! - the grab is let go when the pointer leaves the window, and a later move drags nothing.
//!
//! Nothing here touches the user's windows.

use std::fs;
use std::time::Duration;

use flare_e2e_proto::{Button, PointerAction, Request};
use serde_json::{json, Value};

use super::input_b::{click_on, device_pixel, gap, path_texts, probe, prompt, type_line};
use super::panel::{dev_rect, state, write_picture};
use crate::ctx::{App, Ctx, Launch};
use crate::pixels::Rgb;

const PICTURE_RGB: Rgb = [200, 40, 60];

fn long_text() -> String {
    (0..300)
        .map(|i| format!("line {i} {}\n", "wrapping words ".repeat(24)))
        .collect()
}

fn pass(ctx: &mut Ctx, ms: u64) {
    let t = std::time::Instant::now();
    ctx.until("time to pass", ms + 5_000, &mut || {
        t.elapsed() >= Duration::from_millis(ms)
    });
}

fn pointer(app: &mut App, x: f32, y: f32, action: PointerAction) -> Result<(), String> {
    app.session
        .call(&Request::Pointer { x, y, action })
        .map(|_| ())
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

pub fn panel_splitter(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    write_picture(&alpha.join("split-pic.png"))?;
    fs::write(alpha.join("split-note.txt"), long_text())
        .map_err(|e| format!("make the note: {e}"))?;
    for (dpi, label) in [(96u32, "100%"), (216, "225%")] {
        let mut app = ctx.launch(Launch {
            renderer: Some("gpu"),
            folders: vec![alpha.clone()],
            scale_dpi: Some(dpi),
            ..Launch::default()
        })?;
        let r = splitter_on(ctx, &mut app, dpi, label);
        app.close();
        r?;
    }
    Ok(())
}

/// A pointer path: where each step puts the pointer, in css px from where the press was.
fn line(a: f32, b: f32, n: usize) -> Vec<f32> {
    (0..=n).map(|i| a + (b - a) * i as f32 / n as f32).collect()
}

fn paths() -> Vec<(&'static str, Vec<f32>, u64)> {
    vec![
        ("wider, slow", line(0.0, -150.0, 40), 25),
        ("narrower, quick", line(0.0, 200.0, 6), 5),
        (
            "past the widest and back",
            [line(0.0, -2000.0, 8), line(-2000.0, -40.0, 8)].concat(),
            20,
        ),
        (
            "past the narrowest and back",
            [line(0.0, 2000.0, 8), line(2000.0, 30.0, 8)].concat(),
            20,
        ),
        (
            "shaky",
            (0..40)
                .map(|i| -60.0 + if i % 2 == 0 { 0.0 } else { 1.0 } - (i as f32) * 0.5)
                .collect(),
            15,
        ),
    ]
}

fn splitter_on(ctx: &mut Ctx, app: &mut App, dpi: u32, at: &str) -> Result<(), String> {
    let scale = f64::from(dpi) / 96.0;
    app.session.call(&Request::Resize {
        w: 1000.0,
        h: 640.0,
    })?;
    let sized = ctx.until("the window to be 1000 css px wide", 5_000, &mut || {
        probe(app, "dpi.state", Value::Null)["client_w_px"].as_f64()
            == Some((1000.0 * scale).round())
    });
    ctx.ok(sized, format!("{at}: the window is 1000 css px wide"));
    prompt(app)?;
    type_line(
        app,
        "cls; Write-Host 'picture: split-pic.png note: split-note.txt'",
    )?;
    let painted = ctx.until("both paths to wear the link colour", 8_000, &mut || {
        let t = path_texts(app);
        ["split-pic.png", "split-note.txt"]
            .iter()
            .all(|w| t.iter().any(|p| p.contains(w)))
    });
    ctx.ok(painted, format!("{at}: the picture and the note are links"));

    for (needle, kind, what) in [
        ("split-pic.png", 1, "picture"),
        ("split-note.txt", 2, "text"),
    ] {
        click_on(app, needle, 2, Button::Left)?;
        let shown = ctx.until("the panel to show", 10_000, &mut || {
            let s = state(app);
            s["shown"] == json!(true) && s["kind"] == json!(kind)
        });
        ctx.ok(shown, format!("{at}: the {what} is in the panel"));
        if !shown {
            continue;
        }
        // The window the text settled: the rows are wrapped and the buttons are there.
        if kind == 2 {
            let wrapped = ctx.until("the text to be wrapped", 6_000, &mut || {
                state(app)["wrapped"] == json!(true)
            });
            ctx.ok(wrapped, format!("{at}: the long lines are wrapped"));
            if let Some(w) = dev_rect(app, "panel-wrap") {
                ctx.ok(w.2 > 0, format!("{at}: the header has a wrap button"));
            }
        }
        for (name, xs, wait) in paths() {
            drag(
                ctx,
                app,
                scale,
                kind,
                &format!("{at}, {what}, {name}"),
                &xs,
                wait,
            )?;
        }
        leave(ctx, app, scale, kind, &format!("{at}, {what}"))?;
    }
    Ok(())
}

/// Brightness of the pixels in a small grid over a device rectangle: how many are lighter than `above`.
fn bright(app: &mut App, r: (i64, i64, i64, i64), above: i32) -> usize {
    let mut n = 0;
    for i in 0..48 {
        for j in 0..24 {
            let x = r.0 + 3 + (r.2 - 6).max(1) * i / 48;
            let y = r.1 + 3 + (r.3 - 6).max(1) * j / 24;
            let lit = device_pixel(app, x as f64, y as f64)
                .is_some_and(|p| i32::from(p[0]) + i32::from(p[1]) + i32::from(p[2]) > above * 3);
            if lit {
                n += 1;
            }
        }
    }
    n
}

/// Is the panel's content drawn in the last snapshot? (a picture: its colour in the middle of its box; text: lit
/// pixels in the body.)
fn content_drawn(ctx: &mut Ctx, app: &mut App, kind: i64, tag: &str) -> bool {
    let _ = ctx.shot(app, tag);
    if kind == 1 {
        let Some(p) = dev_rect(app, "panel-image") else {
            return false;
        };
        let mid = device_pixel(app, (p.0 + p.2 / 2) as f64, (p.1 + p.3 / 2) as f64);
        mid.is_some_and(|m| gap(m, PICTURE_RGB) <= 12)
    } else {
        let Some(b) = dev_rect(app, "panel-body") else {
            return false;
        };
        // The left part of the body only: the text starts there, and a very wide slot would dilute the samples.
        let part = (b.0, b.1, b.2.min(320), b.3.min(400));
        bright(app, part, 90) > 6
    }
}

/// The colour beside the slot when it is narrower than the grid: the terminal's ground. Compares a pixel in the
/// gap (between the grid's end and the slot) with one at the grid's far left, low in the window (no text there).
fn gap_ok(app: &mut App, grid_right: i64, slot_left: i64, y: i64, left: i64) -> Option<String> {
    if slot_left - grid_right <= 2 {
        return None;
    }
    let beside = device_pixel(app, (left + 6) as f64, y as f64)?;
    for x in [grid_right + 1, (grid_right + slot_left) / 2, slot_left - 2] {
        let p = device_pixel(app, x as f64, y as f64)?;
        if gap(p, beside) > 9 {
            return Some(format!(
                "the strip beside the slot is {p:?} at x {x}, the terminal's ground is {beside:?}"
            ));
        }
    }
    None
}

fn drag(
    ctx: &mut Ctx,
    app: &mut App,
    scale: f64,
    kind: i64,
    at: &str,
    xs: &[f32],
    wait: u64,
) -> Result<(), String> {
    let s0 = state(app);
    let Some(sp) = dev_rect(app, "splitter") else {
        ctx.ok(false, format!("{at}: the splitter has a rectangle"));
        return Ok(());
    };
    let (x, y) = (
        ((sp.0 + sp.2 / 2) as f64 / scale) as f32,
        ((sp.1 + 200) as f64 / scale) as f32,
    );
    let page_phys = s0["page_phys_w"].as_i64().unwrap_or(0);
    let (min_w, max_w) = (
        s0["layout"]["min_w"].as_i64().unwrap_or(0) as f64 / scale,
        s0["layout"]["max_w"].as_i64().unwrap_or(0) as f64 / scale,
    );
    let w0 = f(&s0["visible_w"]);
    let grid0 = s0["grid_phys_w"].as_i64().unwrap_or(0);
    let fits0 = s0["grid_fits"].as_u64().unwrap_or(0);
    let _ = page_phys;
    let grid_left = dev_rect(app, "grid").map_or(0, |g| g.0);

    pointer(app, x, y, PointerAction::Move)?;
    pointer(
        app,
        x,
        y,
        PointerAction::Down {
            button: Button::Left,
        },
    )?;
    pass(ctx, 40);
    let mut bad: Vec<String> = Vec::new();
    if state(app)["splitting"] != json!(true) {
        bad.push("the press did not start a drag".into());
    }
    let tol = 1.01 / scale;
    let mut last_vis = w0;
    let mut last_x = 0.0f32;
    let step_every = (xs.len() / 6).max(1);
    for (n, dx) in xs.iter().enumerate() {
        pointer(app, x + dx, y, PointerAction::Move)?;
        pass(ctx, wait);
        let s = state(app);
        let vis = f(&s["visible_w"]);
        let want = (f64::from(w0 as f32) - f64::from(*dx)).clamp(min_w, max_w);
        let tag = format!("step {n} dx {dx}");
        if vis < min_w - tol || vis > max_w + tol {
            bad.push(format!(
                "{tag}: the slot is {vis}, outside [{min_w}, {max_w}]"
            ));
        }
        if (vis - want).abs() > tol {
            bad.push(format!("{tag}: the slot is {vis}, the pointer says {want}"));
        }
        let dir = *dx - last_x;
        if dir != 0.0 && (vis - last_vis) * f64::from(dir) > 0.5 {
            bad.push(format!("{tag}: the pointer moved {dir} and the slot went the other way ({last_vis} to {vis})"));
        }
        last_vis = vis;
        last_x = *dx;
        if s["splitting"] != json!(true) {
            bad.push(format!("{tag}: the drag ended by itself"));
        }
        if s["grid_phys_w"].as_i64() != Some(grid0) {
            bad.push(format!(
                "{tag}: the grid changed width ({grid0} to {})",
                s["grid_phys_w"]
            ));
        }
        if s["grid_fits"].as_u64() != Some(fits0) {
            bad.push(format!("{tag}: the grid was fitted during the drag"));
        }
        if let (Some(spl), Some(panel)) = (dev_rect(app, "splitter"), dev_rect(app, "panel")) {
            let mid = spl.0 + spl.2 / 2;
            if (mid - panel.0).abs() > 1 {
                bad.push(format!(
                    "{tag}: the splitter is at {mid}, the slot starts at {}",
                    panel.0
                ));
            }
        }
        // A picture of the window at a few steps and at the extremes.
        if n % step_every == 0 || n + 1 == xs.len() {
            if !content_drawn(ctx, app, kind, &format!("split-{n}")) {
                bad.push(format!("{tag}: the panel's content is not on the frame"));
            }
            if let (Some(panel), Some(g)) = (dev_rect(app, "panel"), dev_rect(app, "grid")) {
                let y = g.1 + (g.3 * 3) / 4;
                if let Some(why) = gap_ok(app, g.0 + grid0, panel.0, y, grid_left) {
                    bad.push(format!("{tag}: {why}"));
                }
            }
        }
    }
    let end = *xs.last().unwrap_or(&0.0);
    pointer(
        app,
        x + end,
        y,
        PointerAction::Up {
            button: Button::Left,
        },
    )?;
    let ended = ctx.until("the drag to end", 4_000, &mut || {
        state(app)["splitting"] == json!(false)
    });
    if !ended {
        bad.push("the release did not end the drag".into());
    }
    pass(ctx, 300);
    let s = state(app);
    let settled = f(&s["slot_phys_w"]) / scale;
    let want = (w0 - f64::from(end)).clamp(min_w, max_w);
    if (settled - want).abs() > tol {
        bad.push(format!(
            "after the release the slot is {settled}, the pointer said {want}"
        ));
    }
    if (f(&s["visible_w"]) - settled).abs() > tol {
        bad.push(format!(
            "after the release the drawn slot is {}, the settled one {settled}",
            s["visible_w"]
        ));
    }
    let moved = (settled - w0).abs() > tol;
    let fitted = s["grid_fits"].as_u64().unwrap_or(0) - fits0;
    if moved && fitted != 1 {
        bad.push(format!(
            "the release fitted the grid {fitted} times, not once"
        ));
    }
    if !moved && fitted != 0 {
        bad.push(format!(
            "nothing moved but the grid was fitted {fitted} times"
        ));
    }
    if let (Some(panel), Some(g)) = (dev_rect(app, "panel"), dev_rect(app, "grid")) {
        if ((g.0 + g.2) - panel.0).abs() > 1 {
            bad.push(format!(
                "after the release the grid ends at {} and the slot starts at {}",
                g.0 + g.2,
                panel.0
            ));
        }
    }
    if !content_drawn(ctx, app, kind, "split-after") {
        bad.push("after the release the panel's content is not on the frame".into());
    }
    if kind == 2 && moved {
        // The rows were wrapped once, to the new width.
        let wrapped = s["wrapped"] == json!(true);
        if !wrapped {
            bad.push("after the release the text is not wrapped".into());
        }
    }
    ctx.ok(
        bad.is_empty(),
        format!(
            "{at}: every invariant held{}",
            if bad.is_empty() {
                String::new()
            } else {
                format!(": {}", bad.join("; "))
            }
        ),
    );
    Ok(())
}

fn leave(ctx: &mut Ctx, app: &mut App, scale: f64, kind: i64, at: &str) -> Result<(), String> {
    let Some(sp) = dev_rect(app, "splitter") else {
        return Ok(());
    };
    let (x, y) = (
        ((sp.0 + sp.2 / 2) as f64 / scale) as f32,
        ((sp.1 + 200) as f64 / scale) as f32,
    );
    let fits0 = state(app)["grid_fits"].as_u64().unwrap_or(0);
    pointer(app, x, y, PointerAction::Move)?;
    pointer(
        app,
        x,
        y,
        PointerAction::Down {
            button: Button::Left,
        },
    )?;
    pointer(app, x - 50.0, y, PointerAction::Move)?;
    pass(ctx, 60);
    let held = state(app)["splitting"] == json!(true);
    pointer(app, x - 50.0, y, PointerAction::Leave)?;
    let released = ctx.until("the grab to be let go", 3_000, &mut || {
        state(app)["splitting"] == json!(false)
    });
    ctx.ok(
        held && released,
        format!("{at}: the pointer leaving the window lets go of the splitter"),
    );
    pass(ctx, 300);
    let w = f(&state(app)["slot_phys_w"]);
    pointer(app, x - 300.0, y, PointerAction::Move)?;
    pass(ctx, 100);
    pointer(
        app,
        x - 300.0,
        y,
        PointerAction::Up {
            button: Button::Left,
        },
    )?;
    pass(ctx, 100);
    let s = state(app);
    ctx.ok(
        f(&s["slot_phys_w"]) == w && s["splitting"] == json!(false),
        format!("{at}: a move and a release after it drag nothing"),
    );
    ctx.ok(
        s["grid_fits"].as_u64().unwrap_or(0) == fits0 + 1,
        format!("{at}: the grid was fitted once for that drag"),
    );
    let drawn = content_drawn(ctx, app, kind, "split-left");
    ctx.ok(
        drawn,
        format!("{at}: the panel is drawn after the grab was let go"),
    );
    Ok(())
}
