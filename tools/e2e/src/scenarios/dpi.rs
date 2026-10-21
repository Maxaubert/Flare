//! The live window at every display scaling and across monitor moves (Task 72): dpiMatrix and
//! monitorMove. Flare's own scenarios (Prism Terminal had no equivalent); the plan's wave 2b section
//! and Task 72 give the checks, `docs/dpi-by-hand.md` the part a machine cannot see.
//!
//! Each scenario launches the app twice, once per renderer, on cmd (a shell whose prompt is the
//! last row and whose output a flood fills fast). A launch without a GPU adapter is a skip with the
//! reason the app gave. Every DPI change is a real `WM_DPICHANGED` through the window procedure
//! (the hook's default mode), and the window stays parked off every monitor throughout.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flare_e2e_proto::{Reply, Request};
use flare_render::metrics::{cell_metrics_at, grid_dims, FontUnits};
use serde_json::{json, Value};

use super::input_d::{epoch_ms, worst_stall};
use crate::ctx::{App, Ctx, Launch};
use crate::session::TabState;
use crate::standins::flood::{flood_file, long_output_cmd};
use crate::world::{prompt_showing, prompt_timeout, rows_text, wait_for_prompt, Shell};

/// The DPIs the matrix walks, 100 to 300 percent (96 dpi is 100 percent).
const DPIS: [u32; 8] = [96, 120, 144, 168, 192, 216, 240, 288];
/// The monitor-move sequence of the plan.
const MOVES: [u32; 8] = [216, 96, 144, 288, 216, 120, 192, 96];
/// The two origins a moved window alternates between, in device px: both far off every monitor.
const ORIGINS: [(i32, i32); 2] = [(-32000, -32000), (-31000, -32000)];
/// The terminal's base font size in logical px (`flare_render::metrics::BASE_FONT_PX`).
const BASE_PX: f32 = 13.0;
/// The title bar's height in css px: the tab strip's bottom line ends there.
const BAR_H: f32 = 36.0;
/// The logical size both renderers start from, in css px.
const START_SIZE: (f32, f32) = (1000.0, 640.0);
/// How many times the 20 MB flood file is typed: long enough to outlast the first four moves
/// even on a fast build (at about 3 MB/s today it also spans all eight).
const FLOOD_REPEATS: u32 = 3;
/// A stall of this many ms in the diag log fails monitorMove.
const STALL_MS: u64 = 1_000;

/// Cascadia Mono's font units, as `tools/pt-metrics` and M1 measured them.
fn cascadia() -> FontUnits {
    FontUnits {
        units_per_em: 2048.0,
        ascent: 1900.0,
        descent: -480.0,
        leading: 0.0,
        advance: 1200.0,
    }
}

fn scale_of(dpi: u32) -> f32 {
    dpi as f32 / 96.0
}

/// The percentage a dpi stands for (`216` is `225`).
fn pct(dpi: u32) -> u32 {
    dpi * 100 / 96
}

/// A hairline's width in device pixels: `max(1, floor(scale))`.
fn hair_px(scale: f32) -> u32 {
    (scale.floor() as u32).max(1)
}

/// What `dpi.state` says, as numbers (0 for a missing field, so a check on it reads false).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct DpiState {
    scale: f32,
    cols: u32,
    rows: u32,
    cell_w: u32,
    cell_h: u32,
    padding: u32,
    client_w: u32,
    client_h: u32,
}

fn parse_dpi_state(v: &Value) -> DpiState {
    let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
    DpiState {
        scale: v.get("scale").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        cols: n("grid_cols"),
        rows: n("grid_rows"),
        cell_w: n("cell_w"),
        cell_h: n("cell_h"),
        padding: n("padding"),
        client_w: n("client_w_px"),
        client_h: n("client_h_px"),
    }
}

fn dpi_state(app: &mut App) -> DpiState {
    parse_dpi_state(&app.session.probe_str("dpi.state").unwrap_or(Value::Null))
}

/// What `render.size` says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RenderSize {
    surface_w: u32,
    surface_h: u32,
    grid_w: u32,
    grid_h: u32,
    tex_w: u32,
    tex_h: u32,
}

fn parse_render_size(v: &Value) -> RenderSize {
    let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
    RenderSize {
        surface_w: n("surface_w"),
        surface_h: n("surface_h"),
        grid_w: n("grid_w"),
        grid_h: n("grid_h"),
        tex_w: n("grid_tex_w"),
        tex_h: n("grid_tex_h"),
    }
}

fn render_size(app: &mut App) -> RenderSize {
    parse_render_size(&app.session.probe_str("render.size").unwrap_or(Value::Null))
}

/// The grid the cell rule gives for a grid area of `w` by `h` device px at `scale`:
/// `(cell_w, cell_h, padding, cols, rows)`, from the font units, never from the app.
fn expected_grid(scale: f32, w: u32, h: u32) -> (u32, u32, u32, usize, usize) {
    let m = cell_metrics_at(&cascadia(), BASE_PX, scale);
    let (cols, rows) = grid_dims(w, h, &m);
    (m.cell_w, m.cell_h, m.padding, cols, rows)
}

/// True when `v` (css px) lands on a whole device pixel at `scale`.
fn on_device_pixel(v: f32, scale: f32) -> bool {
    let d = v * scale;
    (d - d.round()).abs() < 0.02
}

/// The tab edges in device pixels that are not whole, or that overlap or leave a gap wider than a
/// hairline (the separator): the problems with a strip of tabs at `scale`.
fn tab_edge_problems(tabs: &[TabState], scale: f32) -> Vec<String> {
    let mut out = Vec::new();
    for (i, t) in tabs.iter().enumerate() {
        let r = &t.rect;
        for (what, v) in [
            ("left", r.x),
            ("top", r.y),
            ("right", r.x + r.w),
            ("bottom", r.y + r.h),
        ] {
            if !on_device_pixel(v, scale) {
                out.push(format!(
                    "tab {i} {what} edge is {:.3} device px",
                    f64::from(v) * f64::from(scale)
                ));
            }
        }
    }
    let hair = hair_px(scale) as i64;
    for (i, pair) in tabs.windows(2).enumerate() {
        let right = ((pair[0].rect.x + pair[0].rect.w) * scale).round() as i64;
        let left = (pair[1].rect.x * scale).round() as i64;
        let gap = left - right;
        if !(0..=hair).contains(&gap) {
            out.push(format!(
                "tabs {i} and {} are {gap} device px apart (0 to {hair} allowed)",
                i + 1
            ));
        }
    }
    out
}

/// The length of the run of equal colours that ends at the last element of `col` (the pixels of
/// one column, top to bottom, up to and including the last row of the line).
fn run_len(col: &[[u8; 4]]) -> usize {
    match col.last() {
        None => 0,
        Some(last) => col.iter().rev().take_while(|p| *p == last).count(),
    }
}

/// True when a `Frames` reply holds no gap of `limit_ms` or more and at least `min_frames` frames.
fn frames_smooth(r: &Reply, limit_ms: f64, min_frames: u32) -> bool {
    matches!(r, Reply::Frames { max_ms, count, .. } if *max_ms < limit_ms && *count >= min_frames)
}

fn frames(app: &mut App, reset: bool) -> Option<Reply> {
    app.session.call(&Request::Frames { reset }).ok()
}

fn frames_text(r: &Option<Reply>) -> String {
    match r {
        Some(Reply::Frames {
            count,
            max_ms,
            over_33,
            ..
        }) => format!("{count} frames, longest gap {max_ms:.0} ms, {over_33} over 33 ms"),
        _ => "no answer".to_string(),
    }
}

fn front_id(app: &mut App) -> String {
    app.session
        .state()
        .ok()
        .and_then(|s| s.front)
        .unwrap_or_default()
}

/// Types `text` and Enter into the front shell.
fn type_line(app: &mut App, text: &str) -> Result<(), String> {
    app.session.type_text(text)?;
    app.session.key("Enter")
}

/// The window's left and top in device px (`window.park`'s rectangle).
fn window_xy(app: &mut App) -> Option<(i64, i64)> {
    let v = app.session.probe_str("window.park").ok()?;
    let r = v.get("rect")?.as_array()?;
    Some((r.first()?.as_i64()?, r.get(1)?.as_i64()?))
}

/// One pixel of the last snapshot.
fn pixel(app: &mut App, x: u32, y: u32) -> Option<[u8; 4]> {
    match app.session.call(&Request::Pixel { x, y }) {
        Ok(Reply::Pixel { rgba }) => Some(rgba),
        _ => None,
    }
}

/// Seeds the profile so the app's shell is cmd and a launch has no tabs to restore.
fn seed_cmd_shell(profile: &Path) -> Result<(), String> {
    std::fs::create_dir_all(profile).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(profile.join("tabs.json"));
    std::fs::write(
        profile.join("settings.json"),
        json!({"flare.term.shell": "cmd"}).to_string(),
    )
    .map_err(|e| e.to_string())
}

/// Launches the app on `renderer` with two cmd tabs, at 100 percent. `Ok(None)` is a skip that was
/// already recorded (no GPU adapter).
fn start(ctx: &mut Ctx, renderer: &'static str, two_tabs: bool) -> Result<Option<App>, String> {
    let (profile, alpha, beta) = {
        let w = ctx.ensure_world();
        (w.profile.clone(), w.alpha.clone(), w.beta.clone())
    };
    seed_cmd_shell(&profile)?;
    let folders = if two_tabs {
        vec![alpha, beta]
    } else {
        vec![alpha]
    };
    let mut app = ctx.launch(Launch {
        renderer: Some(renderer),
        folders,
        scale_dpi: Some(96),
        ..Launch::default()
    })?;
    if renderer == "gpu" {
        let hello = app.hello()?;
        if !hello.renderer.to_lowercase().starts_with("gpu") {
            let why = app
                .session
                .probe_str("app.state")
                .ok()
                .and_then(|v| v["fallback_reason"].as_str().map(str::to_string))
                .unwrap_or_else(|| hello.renderer.clone());
            ctx.skip(&format!("no GPU adapter ({why})"));
            app.close();
            return Ok(None);
        }
    }
    // Both launches start from the same window, so the two renderers can be compared.
    let _ = app.session.call(&Request::Resize {
        w: START_SIZE.0,
        h: START_SIZE.1,
    });
    let want = device_size(START_SIZE, 96);
    let sized = ctx.until("the start size", 5_000, &mut || {
        let r = render_size(&mut app);
        (r.surface_w, r.surface_h) == want
    });
    ctx.ok(
        sized,
        format!("[{renderer}] the window starts at {want:?} at 100%"),
    );
    let ready = ctx.until("two tabs", 20_000, &mut || {
        app.session
            .state()
            .is_ok_and(|s| s.tabs.len() == if two_tabs { 2 } else { 1 })
    });
    ctx.ok(ready, format!("[{renderer}] the tabs open"));
    let id = front_id(&mut app);
    let prompt = wait_for_prompt(&mut app, &id, Shell::Cmd, prompt_timeout(Shell::Cmd));
    ctx.ok(
        prompt,
        format!("[{renderer}] the cmd shell shows its prompt"),
    );
    Ok(Some(app))
}

/// Quits the app and records that it left by itself with exit 0.
fn finish(ctx: &mut Ctx, mut app: App, renderer: &str) {
    let _ = app.session.call(&Request::Quit { force: false });
    let child = &app.child;
    let left = child.wait_for(|| child.exited().is_some(), Duration::from_secs(10));
    ctx.ok(
        left && app.child.exited() == Some(0),
        format!(
            "[{renderer}] Quit ends the process with exit 0 (left: {left}, code {:?})",
            app.child.exited()
        ),
    );
    if !left {
        app.close();
    }
}

/// One DPI step of dpiMatrix: change, wait, then the ten checks. Returns the grid's `(cols, rows)`.
fn matrix_step(
    ctx: &mut Ctx,
    app: &mut App,
    renderer: &str,
    dpi: u32,
    was: u32,
    home: (i64, i64),
    gpu: bool,
) -> (u32, u32) {
    let scale = scale_of(dpi);
    let at = format!("[{renderer}] at {}%", pct(dpi));
    let _ = frames(app, true);
    let sent = app.session.call(&Request::DpiChange {
        dpi,
        x: None,
        y: None,
    });
    ctx.ok(sent.is_ok(), format!("{at}: the DPI change is accepted"));
    let followed = ctx.until(&format!("the scale to reach {scale}"), 5_000, &mut || {
        (dpi_state(app).scale - scale).abs() < 0.005
    });
    ctx.ok(followed, format!("{at}: the scale follows"));
    ctx.poll(300, &mut || false);
    // 9. no stall while it changed.
    let change = frames(app, false);
    ctx.ok(
        change
            .as_ref()
            .is_some_and(|r| frames_smooth(r, 100.0, u32::from(dpi != was))),
        format!(
            "{at}: no frame gap of 100 ms or more while the scale changed ({})",
            frames_text(&change)
        ),
    );
    // 3 (first half) and 1 and 2: the cell and the grid settle on the rule's numbers.
    let mut st = DpiState::default();
    let mut rs = RenderSize::default();
    let settled = ctx.until(
        &format!("the cell and the grid at {}%", pct(dpi)),
        5_000,
        &mut || {
            st = dpi_state(app);
            rs = render_size(app);
            let (cw, ch, _, cols, rows) = expected_grid(scale, rs.grid_w, rs.grid_h);
            st.cell_w == cw
                && st.cell_h == ch
                && st.cols as usize == cols
                && st.rows as usize == rows
        },
    );
    let (cw, ch, pad, cols, rows) = expected_grid(scale, rs.grid_w, rs.grid_h);
    // 1. the cell metrics are the rule's.
    ctx.ok(
        settled && st.cell_w == cw && st.cell_h == ch && st.padding == pad,
        format!(
            "{at}: the cell is {cw}x{ch} with {pad} px padding (the app has {}x{}, {} px)",
            st.cell_w, st.cell_h, st.padding
        ),
    );
    // 2. the grid fills the grid area in whole cells, exactly `grid_dims`.
    ctx.ok(
        st.cols as usize == cols && st.rows as usize == rows,
        format!(
            "{at}: the grid is {cols}x{rows} whole cells (the app has {}x{})",
            st.cols, st.rows
        ),
    );
    ctx.ok(
        rs.grid_w <= st.client_w && rs.grid_h <= st.client_h && rs.grid_w >= st.client_w / 2,
        format!(
            "{at}: the grid area ({}x{}) lies inside the client ({}x{})",
            rs.grid_w, rs.grid_h, st.client_w, st.client_h
        ),
    );
    // 3. the surface is the client, and the grid texture the grid rectangle.
    ctx.ok(
        rs.surface_w == st.client_w && rs.surface_h == st.client_h,
        format!(
            "{at}: the surface is {}x{} (the client is {}x{})",
            rs.surface_w, rs.surface_h, st.client_w, st.client_h
        ),
    );
    ctx.ok(
        rs.tex_w == rs.grid_w && rs.tex_h == rs.grid_h,
        format!(
            "{at}: the grid texture is {}x{} (the grid rectangle {}x{})",
            rs.tex_w, rs.tex_h, rs.grid_w, rs.grid_h
        ),
    );
    // 9, after it: nothing stalls in the half second that follows.
    let _ = frames(app, true);
    ctx.poll(500, &mut || false);
    let after = frames(app, false);
    let ok_after = if gpu {
        matches!(&after, Some(Reply::Frames { over_33, .. }) if *over_33 == 0)
    } else {
        after.as_ref().is_some_and(|r| frames_smooth(r, 100.0, 0))
    };
    ctx.ok(
        ok_after,
        format!(
            "{at}: no frame stall in the 500 ms after ({})",
            frames_text(&after)
        ),
    );
    // 4. a snapshot is the client in device pixels.
    let shot = ctx
        .shots
        .join(format!("dpiMatrix-{renderer}-{}.png", pct(dpi)));
    let _ = std::fs::create_dir_all(&ctx.shots);
    let snap = app.session.snapshot(&shot);
    ctx.ok(
        snap == Ok((st.client_w, st.client_h)),
        format!(
            "{at}: the snapshot is {:?} (the client is {}x{})",
            snap, st.client_w, st.client_h
        ),
    );
    // 5. hairlines: the app's word, and a run of pixels in the snapshot.
    let hair = app
        .session
        .probe_str("dpi.hairlines")
        .unwrap_or(Value::Null);
    let want = hair_px(scale) as u64;
    ctx.ok(
        hair["strip_bottom_px"].as_u64() == Some(want)
            && hair["tab_separator_px"].as_u64() == Some(want)
            && hair["sidebar_edge_px"].as_u64() == Some(want),
        format!("{at}: the hairlines are {want} device px wide ({hair})"),
    );
    let bar_end = (BAR_H * scale).round() as u32;
    let x = st.client_w / 2;
    let col: Vec<[u8; 4]> = (bar_end.saturating_sub(want as u32 + 3)..bar_end)
        .filter_map(|y| pixel(app, x, y))
        .collect();
    let run = run_len(&col);
    ctx.ok(
        run == want as usize,
        format!("{at}: the strip's bottom line is {run} px in the snapshot (want {want})"),
    );
    // 6. the tabs sit on whole device pixels and meet.
    let tabs = app.session.state().map(|s| s.tabs).unwrap_or_default();
    let problems = tab_edge_problems(&tabs, scale);
    ctx.ok(
        tabs.len() >= 2 && problems.is_empty(),
        format!(
            "{at}: {} tabs, every edge on a device pixel and none overlapping ({})",
            tabs.len(),
            problems.join("; ")
        ),
    );
    // 7. the output and the prompt row survive, and typing echoes at the prompt.
    let id = front_id(app);
    let text = rows_text(app, &id);
    ctx.ok(
        text.contains("dpi-marker-1"),
        format!("{at}: the earlier output (dpi-marker-1) is still on screen"),
    );
    ctx.ok(
        prompt_showing(&text, Shell::Cmd),
        format!("{at}: the prompt row is still the last row"),
    );
    let typed = app.session.type_text("x").is_ok();
    let echoed = typed
        && ctx.until("x to echo at the prompt", 3_000, &mut || {
            rows_text(app, &id).trim_end().ends_with(">x")
        });
    ctx.ok(echoed, format!("{at}: typing x echoes at the prompt"));
    let _ = app.session.key("Backspace");
    let cleared = ctx.until("the x to be erased", 3_000, &mut || {
        prompt_showing(&rows_text(app, &id), Shell::Cmd)
    });
    ctx.ok(cleared, format!("{at}: the prompt is clean again"));
    // 8. the window stayed where it was parked.
    ctx.ok(
        window_xy(app) == Some(home),
        format!(
            "{at}: the window stayed at {home:?} (now {:?})",
            window_xy(app)
        ),
    );
    (st.cols, st.rows)
}

/// One renderer's walk: up the eight scalings and back down. The grid sizes it saw, in order.
fn matrix_one(ctx: &mut Ctx, renderer: &'static str) -> Result<Option<Vec<(u32, u32)>>, String> {
    let Some(mut app) = start(ctx, renderer, true)? else {
        return Ok(None);
    };
    let id = front_id(&mut app);
    type_line(&mut app, "echo dpi-marker-1")?;
    let echoed = ctx.until("the marker", 10_000, &mut || {
        rows_text(&mut app, &id).matches("dpi-marker-1").count() >= 2
    });
    ctx.ok(echoed, format!("[{renderer}] the marker line is printed"));
    let home = window_xy(&mut app).unwrap_or((0, 0));
    let gpu = renderer == "gpu";
    let order: Vec<u32> = DPIS.iter().chain(DPIS.iter().rev()).copied().collect();
    let mut seen = Vec::new();
    // Let the start-up frames pass, then walk from 100%. A step to the scale the window already
    // has draws nothing, so it must show no stall but need not show a frame.
    ctx.poll(1_000, &mut || false);
    let mut was = 96;
    for dpi in order {
        seen.push(matrix_step(ctx, &mut app, renderer, dpi, was, home, gpu));
        was = dpi;
    }
    finish(ctx, app, renderer);
    Ok(Some(seen))
}

/// Flare's own scenario: the live window at every scaling. 10 checks per step, eight scalings up
/// and eight down, on both renderers.
pub fn dpi_matrix(ctx: &mut Ctx) -> Result<(), String> {
    let mut walks: Vec<(&str, Vec<(u32, u32)>)> = Vec::new();
    for renderer in ["software", "gpu"] {
        if let Some(seen) = matrix_one(ctx, renderer)? {
            walks.push((renderer, seen));
        }
    }
    // 10. both renderers see the same grid at the same scale.
    match walks.as_slice() {
        [(_, a), (_, b)] => ctx.ok(
            a == b,
            format!(
                "both renderers report the same cols and rows at every scale ({a:?} against {b:?})"
            ),
        ),
        _ => ctx.skip("the renderers were not compared: only one of them ran"),
    }
    Ok(())
}

/// The longest `Hello` answer in ms over `n` calls spaced 100 ms apart.
fn hello_worst(ctx: &mut Ctx, app: &mut App, n: usize) -> f64 {
    let mut worst: f64 = 0.0;
    for _ in 0..n {
        let t = Instant::now();
        let ok = app.hello().is_ok();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        worst = worst.max(if ok { ms } else { f64::INFINITY });
        ctx.poll(100, &mut || false);
    }
    worst
}

/// The cursor row's text of the front shell.
fn cursor_row(app: &mut App, id: &str) -> String {
    let v = app
        .session
        .probe("grid.text", json!({ "tab": id }))
        .unwrap_or(Value::Null);
    let row = v["cursor"]["row"].as_u64().unwrap_or(u64::MAX) as usize;
    v["rows"]
        .get(row)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// The pure part of a move's settle: the device size a window of `logical` css px has at `dpi`.
fn device_size(logical: (f32, f32), dpi: u32) -> (u32, u32) {
    let s = scale_of(dpi);
    (
        (logical.0 * s).round() as u32,
        (logical.1 * s).round() as u32,
    )
}

fn flood_path(ctx: &mut Ctx) -> PathBuf {
    let root = ctx.ensure_world().root.clone();
    flood_file(&root, 20, "FLOOD-END")
}

/// One renderer of monitorMove.
fn move_one(ctx: &mut Ctx, renderer: &'static str) -> Result<(), String> {
    let Some(mut app) = start(ctx, renderer, false)? else {
        return Ok(());
    };
    let id = front_id(&mut app);
    let from = epoch_ms();
    let st0 = dpi_state(&mut app);
    let mut logical = (
        st0.client_w as f32 / st0.scale.max(0.01),
        st0.client_h as f32 / st0.scale.max(0.01),
    );

    // A flood that outlasts the moves: the 20 MB file, `FLOOD_REPEATS` times.
    let file = flood_path(ctx);
    let f = file.display();
    type_line(
        &mut app,
        &format!("for /L %i in (1,1,{FLOOD_REPEATS}) do @type \"{f}\""),
    )?;
    let started = ctx.until("the flood to start", 10_000, &mut || {
        rows_text(&mut app, &id).contains("LINE ")
    });
    ctx.ok(started, format!("[{renderer}] the flood is running"));

    for (i, dpi) in MOVES.iter().copied().enumerate() {
        let (x, y) = ORIGINS[i % 2];
        if i == 4 {
            // A resize between two moves, as dragging an edge would.
            logical = (900.0, 600.0);
            let sent = app.session.call(&Request::Resize {
                w: logical.0,
                h: logical.1,
            });
            ctx.ok(
                sent.is_ok(),
                format!("[{renderer}] a Resize to 900x600 is accepted"),
            );
            let want = device_size(logical, MOVES[i - 1]);
            let sized = ctx.until("the resize to settle", 5_000, &mut || {
                let r = render_size(&mut app);
                (r.surface_w, r.surface_h) == want
            });
            ctx.ok(
                sized,
                format!("[{renderer}] the resize settles at {want:?}"),
            );
        }
        let at = format!("[{renderer}] move {} to {}% at ({x}, {y})", i + 1, pct(dpi));
        let sent = app.session.call(&Request::DpiChange {
            dpi,
            x: Some(x),
            y: Some(y),
        });
        ctx.ok(sent.is_ok(), format!("{at}: accepted"));
        let want = device_size(logical, dpi);
        let mut got = RenderSize::default();
        let settled = ctx.until(
            &format!("the exact device size {want:?}"),
            5_000,
            &mut || {
                got = render_size(&mut app);
                (got.surface_w, got.surface_h) == want
                    && (dpi_state(&mut app).scale - scale_of(dpi)).abs() < 0.005
            },
        );
        ctx.ok(
            settled,
            format!(
                "{at}: the surface settles at {want:?} (now {}x{})",
                got.surface_w, got.surface_h
            ),
        );
        ctx.ok(
            app.hello().is_ok(),
            format!("{at}: the app still answers Hello"),
        );
        // A picture of every move for the reader of the scaling images (Task 73); failures are ignored.
        let _ = ctx.shot(&mut app, &format!("{renderer}-move{}-{}", i + 1, pct(dpi)));
        if i == 3 {
            // Through the 300% move at least, the output was still flowing (else it proved nothing).
            ctx.ok(
                !prompt_showing(&rows_text(&mut app, &id), Shell::Cmd),
                format!("[{renderer}] the flood was still running after the fourth move"),
            );
        }
    }

    // The flood ends, whatever the window went through. The closing line wraps on a narrow grid, so the rows are
    // joined before FLOOD-END is looked for.
    let done = ctx.until("FLOOD-END and a prompt", 120_000, &mut || {
        let t = rows_text(&mut app, &id);
        t.replace('\n', "").contains("FLOOD-END") && prompt_showing(&t, Shell::Cmd)
    });
    let shown = rows_text(&mut app, &id);
    let tail: Vec<&str> = shown.lines().rev().take(3).collect();
    ctx.ok(
        done,
        format!(
            "[{renderer}] the flood finishes (FLOOD-END, then the prompt); the last rows: {tail:?}"
        ),
    );
    type_line(&mut app, "echo after-move")?;
    let echoed = ctx.until("after-move", 10_000, &mut || {
        rows_text(&mut app, &id).matches("after-move").count() >= 2
    });
    ctx.ok(
        echoed,
        format!("[{renderer}] the prompt line survives (echo after-move)"),
    );
    let _ = ctx.shot(&mut app, &format!("{renderer}-after-move"));

    // The roadmap's runaway output: Ctrl+C reaches the shell and the window keeps up.
    let _ = frames(&mut app, true);
    type_line(&mut app, &long_output_cmd(10_000_000))?;
    ctx.poll(1_000, &mut || false);
    let worst = hello_worst(ctx, &mut app, 8);
    ctx.ok(
        worst <= 100.0,
        format!("[{renderer}] every Hello during the runaway output was answered within 100 ms (worst {worst:.0} ms)"),
    );
    let t = Instant::now();
    let probed = app.session.probe_str("dpi.state").is_ok();
    ctx.ok(
        probed && t.elapsed() < Duration::from_millis(500),
        format!(
            "[{renderer}] the UI thread answers a probe during it ({} ms)",
            t.elapsed().as_millis()
        ),
    );
    let sent = app.session.key("Ctrl+C").is_ok();
    ctx.ok(sent, format!("[{renderer}] Ctrl+C is pressed"));
    let back = ctx.until("the prompt after Ctrl+C", 5_000, &mut || {
        cursor_row(&mut app, &id).trim_end().ends_with('>')
    });
    ctx.ok(
        back,
        format!("[{renderer}] Ctrl+C brings the prompt back within 5 s"),
    );
    ctx.ok(
        !rows_text(&mut app, &id).contains("line 10000000"),
        format!("[{renderer}] the runaway output never reached its last line"),
    );
    if renderer == "gpu" {
        let fr = frames(&mut app, false);
        ctx.ok(
            fr.as_ref().is_some_and(|r| frames_smooth(r, 100.0, 1)),
            format!(
                "[gpu] no frame gap of 100 ms or more over the flood and the runaway output ({})",
                frames_text(&fr)
            ),
        );
    }

    let stall = worst_stall(
        &flare_diag::reader::read_lines(&app.profile.join("logs")),
        from,
    );
    ctx.ok(
        stall < STALL_MS,
        format!(
            "[{renderer}] the diag log holds no stall of {STALL_MS} ms or more (worst {stall} ms)"
        ),
    );
    finish(ctx, app, renderer);
    Ok(())
}

/// Flare's own scenario: a flood of output while the window is moved between monitors of different
/// scalings, then a runaway output stopped with Ctrl+C.
pub fn monitor_move(ctx: &mut Ctx) -> Result<(), String> {
    for renderer in ["software", "gpu"] {
        move_one(ctx, renderer)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Rect;

    fn tab(x: f32, w: f32) -> TabState {
        TabState {
            rect: Rect {
                x,
                y: 4.0,
                w,
                h: 28.0,
            },
            ..TabState::default()
        }
    }

    #[test]
    fn the_percentages_are_the_windows_ones() {
        let all: Vec<u32> = DPIS.iter().map(|d| pct(*d)).collect();
        assert_eq!(all, [100, 125, 150, 175, 200, 225, 250, 300]);
    }

    #[test]
    fn a_hairline_is_the_floor_of_the_scale_and_at_least_one() {
        assert_eq!(hair_px(1.0), 1);
        assert_eq!(hair_px(1.25), 1);
        assert_eq!(hair_px(2.25), 2);
        assert_eq!(hair_px(3.0), 3);
    }

    #[test]
    fn the_expected_grid_changes_with_the_scale() {
        let (cw1, ch1, _, c1, r1) = expected_grid(1.0, 1000, 600);
        let (cw3, ch3, _, c3, r3) = expected_grid(3.0, 3000, 1800);
        assert_eq!((cw1, ch1), (8, 16));
        assert!(cw3 > cw1 && ch3 > ch1);
        assert!(c1 > 0 && r1 > 0 && c3 > 0 && r3 > 0);
    }

    #[test]
    fn a_grid_that_kept_the_old_cell_size_is_not_the_rule() {
        let (cw, ch, ..) = expected_grid(2.25, 2250, 1350);
        let (ow, oh, ..) = expected_grid(1.0, 2250, 1350);
        assert_ne!((cw, ch), (ow, oh));
    }

    #[test]
    fn tab_edges_must_be_whole_device_pixels_and_meet() {
        let good = [tab(10.0, 100.0), tab(111.0, 100.0)];
        assert!(tab_edge_problems(&good, 1.0).is_empty());
        // 100.4 css px at 1.0 is not a whole device pixel.
        let fractional = [tab(10.0, 100.4), tab(111.0, 100.0)];
        assert_eq!(tab_edge_problems(&fractional, 1.0).len(), 1);
        let overlap = [tab(10.0, 100.0), tab(105.0, 100.0)];
        assert_eq!(tab_edge_problems(&overlap, 1.0).len(), 1);
        let gap = [tab(10.0, 100.0), tab(130.0, 100.0)];
        assert_eq!(tab_edge_problems(&gap, 1.0).len(), 1);
    }

    #[test]
    fn a_run_is_counted_up_from_the_last_row() {
        let a = [10, 10, 10, 255];
        let b = [40, 40, 40, 255];
        assert_eq!(run_len(&[a, a, b, b]), 2);
        assert_eq!(run_len(&[a, b, b, b]), 3);
        assert_eq!(run_len(&[a, a, a, a]), 4);
        assert_eq!(run_len(&[]), 0);
    }

    #[test]
    fn smooth_frames_need_the_gap_and_the_count() {
        let r = |count, max_ms| Reply::Frames {
            count,
            mean_ms: 0.0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            p99_ms: 0.0,
            max_ms,
            over_33: 0,
            render_p95_ms: 0.0,
        };
        assert!(frames_smooth(&r(3, 40.0), 100.0, 1));
        assert!(!frames_smooth(&r(3, 140.0), 100.0, 1));
        assert!(!frames_smooth(&r(0, 0.0), 100.0, 1));
        assert!(frames_smooth(&r(0, 0.0), 100.0, 0));
    }

    #[test]
    fn a_move_settles_at_the_rounded_device_size() {
        assert_eq!(device_size((1000.0, 640.0), 216), (2250, 1440));
        assert_eq!(device_size((1000.0, 640.0), 96), (1000, 640));
        assert_eq!(device_size((900.0, 600.0), 120), (1125, 750));
    }

    #[test]
    fn the_move_sequence_is_the_plans() {
        assert_eq!(MOVES, [216, 96, 144, 288, 216, 120, 192, 96]);
        assert_eq!(ORIGINS[0], (-32000, -32000));
    }
}

// ---- resizeEdges: the frameless window's native resize handles ------------------------------------

/// `HTCLIENT`, `HTLEFT`..`HTBOTTOMRIGHT` as `WM_NCHITTEST` answers them.
const HT_CLIENT: u32 = 1;
const HT_LEFT: u32 = 10;
const HT_RIGHT: u32 = 11;
const HT_TOP: u32 = 12;
const HT_TOPLEFT: u32 = 13;
const HT_TOPRIGHT: u32 = 14;
const HT_BOTTOM: u32 = 15;
const HT_BOTTOMLEFT: u32 = 16;
const HT_BOTTOMRIGHT: u32 = 17;
/// The window's minimum size in css px (Prism Terminal's `minWidth` x `minHeight`).
const MIN_CSS: (f32, f32) = (560.0, 400.0);

/// The points of a `w` x `h` window (device px, from its top-left) a resize band is checked at,
/// with the code each answers: the middle of each edge, each corner's outermost pixel and reach, a
/// point just inside a band, the middle, and, when `close` is given (the Close button's own last
/// pixel, mid-height), that pixel (client, not a handle).
fn edge_points(
    w: i32,
    h: i32,
    band: i32,
    corner: i32,
    close: Option<(i32, i32)>,
) -> Vec<((i32, i32), u32, &'static str)> {
    let mut pts = vec![
        ((0, h / 2), HT_LEFT, "left edge"),
        ((w - 1, h / 2), HT_RIGHT, "right edge"),
        ((w / 2, 0), HT_TOP, "top edge"),
        ((w / 2, h - 1), HT_BOTTOM, "bottom edge"),
        ((0, 0), HT_TOPLEFT, "top-left corner"),
        ((w - 1, 0), HT_TOPRIGHT, "top-right corner"),
        ((0, h - 1), HT_BOTTOMLEFT, "bottom-left corner"),
        ((w - 1, h - 1), HT_BOTTOMRIGHT, "bottom-right corner"),
        (
            (corner - 1, 0),
            HT_TOPLEFT,
            "the corner's last pixel along the top",
        ),
        (
            (corner, 0),
            HT_TOP,
            "one pixel past the corner along the top",
        ),
        (
            (0, corner - 1),
            HT_TOPLEFT,
            "the corner's last pixel down the left",
        ),
        (
            (0, corner),
            HT_LEFT,
            "one pixel past the corner down the left",
        ),
        ((w - band, h / 2), HT_RIGHT, "the right band's inner pixel"),
        (
            (w - band - 1, h / 2),
            HT_CLIENT,
            "one pixel inside the right band",
        ),
        ((band - 1, h / 2), HT_LEFT, "the left band's inner pixel"),
        ((band, h / 2), HT_CLIENT, "one pixel inside the left band"),
        ((w / 2, band), HT_CLIENT, "one pixel inside the top band"),
        ((w / 2, h / 2), HT_CLIENT, "the middle"),
    ];
    if let Some(p) = close {
        pts.push((p, HT_CLIENT, "the Close button's last pixel"));
    }
    pts
}

/// The band Prism Terminal's frameless window has at a dpi, as literals: `round(5 * dpi / 96)` for
/// an edge and `round(16 * dpi / 96)` for a corner (100, 125, 150, 175, 200, 225, 250, 300 percent).
fn want_band_at(dpi: u32) -> Option<(i64, i64)> {
    Some(match dpi {
        96 => (5, 16),
        120 => (6, 20),
        144 => (8, 24),
        168 => (9, 28),
        192 => (10, 32),
        216 => (11, 36),
        240 => (13, 40),
        288 => (15, 48),
        _ => return None,
    })
}

/// `window.edges` for the points, as `(rect, dpi, band, sizebox, min_track, codes)`.
type Edges = ([i64; 4], u32, (i64, i64), bool, (i64, i64), Vec<u32>);

fn window_edges(app: &mut App, points: &[(i32, i32)]) -> Option<Edges> {
    let pts: Vec<Value> = points.iter().map(|p| json!([p.0, p.1])).collect();
    let v = app
        .session
        .probe("window.edges", json!({ "points": pts }))
        .ok()?;
    let n = |x: &Value| x.as_i64();
    let r = v["rect"].as_array()?;
    let rect = [n(&r[0])?, n(&r[1])?, n(&r[2])?, n(&r[3])?];
    let m = v["min_track"].as_array()?;
    let codes = v["codes"]
        .as_array()?
        .iter()
        .map(|c| c.as_u64().unwrap_or(0) as u32)
        .collect();
    Some((
        rect,
        v["dpi"].as_u64()? as u32,
        (n(&v["band"][0])?, n(&v["corner"])?),
        v["sizebox"].as_bool()?,
        (n(&m[0])?, n(&m[1])?),
        codes,
    ))
}

/// Flare's own scenario: the frameless main window has native resize edges and corners at every
/// scaling, answers the right `HT*` codes through its real window procedure, keeps the client
/// everywhere else, and is held to its minimum size. The window stays parked.
pub fn resize_edges(ctx: &mut Ctx) -> Result<(), String> {
    let Some(mut app) = start(ctx, "software", false)? else {
        return Ok(());
    };
    let mut checked_close = false;
    let mut window_dpi = 96;
    for dpi in [96u32, 144, 216, 288] {
        let at = format!("at {}%", pct(dpi));
        let sent = app.session.call(&Request::DpiChange {
            dpi,
            x: None,
            y: None,
        });
        ctx.ok(sent.is_ok(), format!("{at}: the DPI change is accepted"));
        let scale = scale_of(dpi);
        let followed = ctx.until(&format!("the scale to reach {scale}"), 5_000, &mut || {
            (dpi_state(&mut app).scale - scale).abs() < 0.005
        });
        ctx.ok(followed, format!("{at}: the scale follows"));
        ctx.poll(300, &mut || false);
        let Some((rect, got_dpi, band, sizebox, min, _)) = window_edges(&mut app, &[]) else {
            ctx.ok(false, format!("{at}: window.edges answers"));
            continue;
        };
        let (w, h) = ((rect[2] - rect[0]) as i32, (rect[3] - rect[1]) as i32);
        window_dpi = got_dpi;
        ctx.ok(
            sizebox,
            format!("{at}: the window can be sized (WS_SIZEBOX, so Snap works)"),
        );
        // The band follows the WINDOW's own DPI, which the simulated change does not move (it moves
        // the app's scale only): every step reads the host monitor's band, Prism Terminal's 5 px
        // edges and 16 px corners per 96 dpi, from literals. The sizes at 100 to 300 percent are
        // held by the unit tests (`the_band_scales_with_the_dpi_from_100_to_300_percent`).
        let (want_band, want_corner) = want_band_at(got_dpi).unwrap_or_else(|| {
            let s = f64::from(got_dpi) / 96.0;
            ((5.0 * s).round() as i64, (16.0 * s).round() as i64)
        });
        ctx.ok(
            band.0 == want_band && band.1 == want_corner,
            format!(
                "{at} (the window stays at the monitor's {got_dpi} dpi): the edge is {} px and the corner {} px (want {want_band} and {want_corner})",
                band.0, band.1
            ),
        );
        let want_min = (
            (MIN_CSS.0 * scale).round() as i64,
            (MIN_CSS.1 * scale).round() as i64,
        );
        ctx.ok(
            (min.0 - want_min.0).abs() <= 1 && (min.1 - want_min.1).abs() <= 1,
            format!("{at}: the minimum is {min:?} (about {want_min:?})"),
        );
        // The Close button's last pixel: the title buttons pad 6 css px on the right and the bar is
        // 36 css px, so the pixel is (w - round(6 * scale) - 1, round(18 * scale)). The layout
        // follows the app's scale, so this is only asserted where that equals the window's own
        // scale (else the band, from the window's dpi, and the layout disagree by construction).
        let close = ((got_dpi as f32 / 96.0 - scale).abs() < 0.005).then(|| {
            (
                w - (6.0 * scale).round() as i32 - 1,
                (18.0 * scale).round() as i32,
            )
        });
        checked_close |= close.is_some();
        let pts = edge_points(w, h, band.0 as i32, band.1 as i32, close);
        let xy: Vec<(i32, i32)> = pts.iter().map(|p| p.0).collect();
        let codes = window_edges(&mut app, &xy).map(|e| e.5).unwrap_or_default();
        for (i, (_, want, what)) in pts.iter().enumerate() {
            let got = codes.get(i).copied();
            ctx.ok(
                got == Some(*want),
                format!("{at}: {what} answers {want} (got {got:?})"),
            );
        }
    }
    // The Close button's own last pixel is client, not a resize edge, at the step whose scale is the
    // window's (any monitor at 100, 150, 225 or 300 percent has one).
    ctx.ok(
        checked_close || ![96, 144, 216, 288].contains(&window_dpi),
        format!("the Close button's last pixel was probed at the step matching the window's {window_dpi} dpi"),
    );
    // Back to 100 percent, then a programmatic resize far below the minimum is held to it.
    let _ = app.session.call(&Request::DpiChange {
        dpi: 96,
        x: None,
        y: None,
    });
    ctx.poll(500, &mut || false);
    let _ = app.session.call(&Request::Resize { w: 100.0, h: 100.0 });
    let clamped = ctx.until("the clamp to the minimum", 5_000, &mut || {
        window_edges(&mut app, &[]).is_some_and(|e| {
            (e.0[2] - e.0[0], e.0[3] - e.0[1]) == (MIN_CSS.0 as i64, MIN_CSS.1 as i64)
        })
    });
    let size = window_edges(&mut app, &[]).map(|e| (e.0[2] - e.0[0], e.0[3] - e.0[1]));
    ctx.ok(
        clamped,
        format!("a resize to 100 x 100 is held at the minimum 560 x 400 (got {size:?})"),
    );
    finish(ctx, app, "software");
    Ok(())
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    #[test]
    fn the_edge_points_lie_inside_the_window_and_cover_every_handle() {
        for (w, h, band, corner) in [(1000, 640, 5, 16), (3000, 1920, 15, 48)] {
            let pts = edge_points(w, h, band, corner, Some((w - 7, h / 20)));
            let codes: Vec<u32> = pts.iter().map(|p| p.1).collect();
            for want in [
                HT_LEFT,
                HT_RIGHT,
                HT_TOP,
                HT_BOTTOM,
                HT_TOPLEFT,
                HT_TOPRIGHT,
                HT_BOTTOMLEFT,
                HT_BOTTOMRIGHT,
                HT_CLIENT,
            ] {
                assert!(codes.contains(&want), "{want}");
            }
            for ((x, y), _, what) in pts {
                assert!((0..w).contains(&x) && (0..h).contains(&y), "{what}");
            }
        }
    }
}
