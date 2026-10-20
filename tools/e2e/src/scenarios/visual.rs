//! `visualTour` (audit 2026-10-20): the visual comparison of Flare against Prism Terminal, Flare's half.
//!
//! It writes PNG frame sequences and per-frame numbers for four surfaces, every capture taken while the
//! window stays parked offscreen (the harness's own guards still hold: `--e2e`, unfocusable, a world of its own):
//!
//! - `startup`: launch to the first full frame. The window is copied from OUTSIDE with `PrintWindow` from
//!   process creation (before the pipe exists), each changed picture is kept with its time, and the app's own
//!   `startup.trace` phases and per-frame series are read once the pipe opens. Native scale only: the app starts
//!   at the monitor's scale and a `DpiChange` can only follow the connection.
//! - `resume`: a saved session (three tabs, the vertical sidebar open, the middle tab in front) coming back.
//! - `sidebar`: the pinned sidebar collapsing and expanding (Ctrl+Shift+B): the per-frame series of the slot
//!   width and the panel x (frames, longest gap, the curve against the declared ease) and snapshots at fixed
//!   offsets into the slide.
//! - `general`: the tab strip, the tab menu, Settings, the command help, a theme switch and see-through; and
//!   `dictation`: the pill while a clip is heard.
//!
//! Output: `<out>/<surface>/<pct>-<renderer>/` holds `frames/NN-*.png`, `numbers.json` and (made by
//! `tools/visual/sheet.py`) `sheet.png`. Which cells and surfaces run is chosen by the environment:
//! `FLARE_VISUAL_OUT` (default `docs/screenshots/visual`), `FLARE_VISUAL_CELLS` (`all`, or `225-gpu,100-software`),
//! `FLARE_VISUAL_SURFACES` (`all`, or `sidebar,general`) and `FLARE_VISUAL_RUNS` (start-up timing launches, 5).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flare_e2e_proto::Request;
use serde_json::{json, Value};

use super::driver;
use crate::ctx::{park_problems, App, Ctx, Launch};
use crate::perf::{median, pause};
use crate::proc::Child;
use crate::session::Session;
use crate::slide;
use crate::tour::{judge_frames, num, probe, wait_until, wake_display, watch_launch, Tabs};
use crate::world::{prompt_showing, prompt_timeout, rows_text, wait_for_prompt, Shell, World};

/// The window the tour sizes everything to, css px.
const SIZE: (f32, f32) = (1000.0, 640.0);
/// The scalings of the matrix, as percent and the DPI that sets them.
const SCALES: [(u32, u32); 3] = [(100, 96), (150, 144), (225, 216)];
/// Scalings only a cell list that names them runs (`FLARE_VISUAL_CELLS=300-gpu`); `all` leaves them out,
/// so the committed matrix and Prism Terminal's pictures keep their three.
const OPT_IN_SCALES: [(u32, u32); 1] = [(300, 288)];
const RENDERERS: [&str; 2] = ["gpu", "software"];
/// The offsets into a slide the snapshots are taken at, ms after the key.
const OFFSETS: [u64; 10] = [0, 20, 40, 60, 80, 100, 130, 160, 200, 260];

// ---- the configuration -----------------------------------------------------------------------------

struct Cfg {
    out: PathBuf,
    cells: Vec<(u32, u32, &'static str)>,
    surfaces: Vec<String>,
    runs: usize,
}

impl Cfg {
    fn wants(&self, surface: &str) -> bool {
        self.surfaces.iter().any(|s| s == surface)
    }

    fn dir(&self, surface: &str, pct: u32, renderer: &str) -> PathBuf {
        self.out.join(surface).join(format!("{pct}-{renderer}"))
    }
}

/// Splits a comma list; `all` or nothing is `None` (no filter).
pub fn list_filter(text: Option<&str>) -> Option<Vec<String>> {
    let t = text?.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("all") {
        return None;
    }
    Some(
        t.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

fn cfg() -> Cfg {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let out = std::env::var("FLARE_VISUAL_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| repo.join("docs").join("screenshots").join("visual"));
    let want_cells = list_filter(std::env::var("FLARE_VISUAL_CELLS").ok().as_deref());
    let mut cells = Vec::new();
    for (pct, dpi) in SCALES.iter().chain(OPT_IN_SCALES.iter()).copied() {
        for r in RENDERERS {
            let name = format!("{pct}-{r}");
            let named = want_cells.as_ref().is_some_and(|w| w.contains(&name));
            let opt_in = OPT_IN_SCALES.iter().any(|(p, _)| *p == pct);
            if named || (!opt_in && want_cells.is_none()) {
                cells.push((pct, dpi, r));
            }
        }
    }
    let surfaces = list_filter(std::env::var("FLARE_VISUAL_SURFACES").ok().as_deref())
        .unwrap_or_else(|| {
            ["startup", "resume", "sidebar", "general", "dictation"]
                .iter()
                .map(|s| (*s).to_string())
                .collect()
        });
    let runs = std::env::var("FLARE_VISUAL_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    Cfg {
        out,
        cells,
        surfaces,
        runs,
    }
}

// ---- small tools -----------------------------------------------------------------------------------

/// Writes `settings.json` into a closed profile (a JSON object of strings).
fn seed(profile: &Path, pairs: &[(&str, &str)]) -> Result<(), String> {
    fs::create_dir_all(profile).map_err(|e| e.to_string())?;
    let map: serde_json::Map<String, Value> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), json!(v)))
        .collect();
    fs::write(
        profile.join("settings.json"),
        serde_json::to_string(&map).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// `tabs.json` for a saved session: these folders, the tab at `active` in front.
fn seed_tabs(profile: &Path, folders: &[&Path], active: usize) -> Result<(), String> {
    let tabs: Vec<Value> = folders
        .iter()
        .map(|f| json!({ "cwd": f.to_string_lossy() }))
        .collect();
    fs::write(
        profile.join("tabs.json"),
        json!({ "tabs": tabs, "active": active }).to_string(),
    )
    .map_err(|e| e.to_string())
}

/// A third folder beside the world's `alpha` and `beta`.
fn gamma(w: &World) -> PathBuf {
    let g = w.root.join("gamma");
    let _ = fs::create_dir_all(&g);
    g
}

fn rect_c(v: &Value) -> Option<(f32, f32)> {
    Some((
        (v["x"].as_f64()? + v["w"].as_f64()? / 2.0) as f32,
        (v["y"].as_f64()? + v["h"].as_f64()? / 2.0) as f32,
    ))
}

fn tab_count(app: &mut App) -> usize {
    Tabs::read(app).map_or(0, |t| t.len())
}

fn front_id(app: &mut App) -> String {
    Tabs::read(app).and_then(|t| t.front).unwrap_or_default()
}

fn save_numbers(dir: &Path, v: &Value) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    fs::write(
        dir.join("numbers.json"),
        serde_json::to_string_pretty(v).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// The slot and panel x the last snapshot's picture shows (the app reads them right after it takes the
/// picture), `None` when the app did not say.
fn picture_slide(app: &mut App) -> Option<(f64, f64)> {
    let series = app.session.probe_str("frames.series").ok()?;
    let shown = series.get("snap_slide")?.as_array()?;
    Some((shown.first()?.as_f64()?, shown.get(1)?.as_f64()?))
}

fn shot_to(app: &mut App, path: &Path) -> bool {
    if let Some(d) = path.parent() {
        let _ = fs::create_dir_all(d);
    }
    app.session.snapshot(path).is_ok()
}

/// Launches one cell's app: its own world, the renderer, the scale, the size; `Err` carries the reason
/// the cell cannot run (no GPU, say), which the caller records as a skip.
fn start(
    ctx: &mut Ctx,
    w: &World,
    renderer: &'static str,
    dpi: Option<u32>,
    folders: Vec<PathBuf>,
    env: Vec<(String, String)>,
) -> Result<App, String> {
    let mut app = ctx.launch_in(
        w,
        Launch {
            args: vec!["--no-import".into()],
            env,
            folders,
            renderer: Some(renderer),
            scale_dpi: dpi,
            ..Launch::default()
        },
    )?;
    if renderer == "gpu" {
        let hello = app.hello()?;
        if !hello.renderer.to_lowercase().starts_with("gpu") {
            return Err(format!("no GPU adapter ({})", hello.renderer));
        }
    }
    app.session.call(&Request::Resize {
        w: SIZE.0,
        h: SIZE.1,
    })?;
    // Its own focus event, as a person returning to it would send, so a field shows a caret.
    let _ = app.session.call(&Request::Focus { on: true });
    wake_display();
    Ok(app)
}

fn ready(ctx: &mut Ctx, app: &mut App, tabs: usize) -> bool {
    let opened = ctx.until("the tabs to open", 20_000, &mut || tab_count(app) == tabs);
    if !opened {
        let seen = tab_count(app);
        println!("  note  {tabs} tabs were wanted, the app shows {seen:?}");
    }
    let id = front_id(app);
    opened && wait_for_prompt(app, &id, Shell::Cmd, prompt_timeout(Shell::Cmd)) && {
        pause(300);
        true
    }
}

// ---- the sidebar --------------------------------------------------------------------------------------

struct Toggle {
    rows: Vec<slide::Row>,
    press_age: f64,
}

/// Resets the frame series, presses Ctrl+Shift+B, waits until the sidebar is `open` and quiet, and
/// returns the frames the slide drew.
fn toggle(ctx: &mut Ctx, app: &mut App, open: bool) -> Result<Toggle, String> {
    app.session.call(&Request::Frames { reset: true })?;
    let before = app.session.probe_str("frames.series")?;
    let press_age = num(&before["now_age_ms"]);
    app.session.key("Ctrl+Shift+B")?;
    let moved = ctx.until("the sidebar to change state", 4_000, &mut || {
        let s = probe(app, "sidebar.state", Value::Null);
        s["open"] == json!(open) && s["sliding"] == json!(false)
    });
    if !moved {
        return Err(format!("the sidebar never became open={open}"));
    }
    pause(450);
    let series = app.session.probe_str("frames.series")?;
    Ok(Toggle {
        rows: slide::parse_rows(&series),
        press_age,
    })
}

fn median_of(runs: &[Value], key: &str) -> f64 {
    let v: Vec<f64> = runs.iter().filter_map(|r| r[key].as_f64()).collect();
    median(&v)
}

/// The pinned sidebar collapsing and expanding in one cell.
fn sidebar_cell(
    ctx: &mut Ctx,
    cfg: &Cfg,
    pct: u32,
    dpi: u32,
    renderer: &'static str,
) -> Result<(), String> {
    let dir = cfg.dir("sidebar", pct, renderer);
    let _ = fs::remove_dir_all(dir.join("frames"));
    let w = ctx.new_world();
    seed(
        &w.profile,
        &[
            ("flare.term.shell", "cmd"),
            ("flare.window.tabLayout", "vertical"),
            ("flare.window.sidebar", "open"),
        ],
    )?;
    // Like Prism Terminal's sidebar cell: a saved session of alpha, beta and gamma with the middle tab
    // selected (its `prepResume` writes `active: 1`), so both apps show the same tab in front.
    let g = gamma(&w);
    seed_tabs(&w.profile, &[&w.alpha, &w.beta, &g], 1)?;
    let mut app = start(ctx, &w, renderer, Some(dpi), Vec::new(), Vec::new())?;
    let ok = ready(ctx, &mut app, 3);
    ctx.ok(
        ok,
        format!("sidebar {pct}-{renderer}: three tabs and a prompt"),
    );
    if !ok {
        app.close();
        return Ok(());
    }
    let front = Tabs::read(&mut app).and_then(|t| t.front_index());
    ctx.ok(
        front == Some(1),
        format!("sidebar {pct}-{renderer}: the saved middle tab is in front, as in Prism Terminal ({front:?})"),
    );
    let open = ctx.until("the sidebar open", 6_000, &mut || {
        probe(&mut app, "sidebar.state", Value::Null)["open"] == json!(true)
    });
    ctx.ok(
        open,
        format!("sidebar {pct}-{renderer}: it starts pinned open"),
    );
    pause(500);
    let cols_open = num(&probe(&mut app, "dpi.state", Value::Null)["grid_cols"]);
    let hello = app.hello()?;

    // The numbers: three collapses and three expands, the series of each.
    let mut collapse = Vec::new();
    let mut expand = Vec::new();
    for _ in 0..3 {
        for (open_after, list) in [(false, &mut collapse), (true, &mut expand)] {
            let t = toggle(ctx, &mut app, open_after)?;
            let slot = slide::analyse(&t.rows, slide::slot, t.press_age);
            let left = slide::analyse(&t.rows, slide::left, t.press_age);
            list.push(json!({
                "slot": slot.map(|s| s.to_json()),
                "panel": left.map(|s| s.to_json()),
                "frames_total": t.rows.len(),
                "rows": t.rows.iter().take(120).map(|r| json!([r.age, r.gap, r.render, r.slot, r.left])).collect::<Vec<_>>(),
            }));
        }
    }
    let cols_collapsed = {
        toggle(ctx, &mut app, false)?;
        let c = num(&probe(&mut app, "dpi.state", Value::Null)["grid_cols"]);
        toggle(ctx, &mut app, true)?;
        c
    };
    let cols_back = num(&probe(&mut app, "dpi.state", Value::Null)["grid_cols"]);
    ctx.ok(
        collapse.iter().all(|r| !r["slot"].is_null())
            && expand.iter().all(|r| !r["slot"].is_null()),
        format!("sidebar {pct}-{renderer}: every slide moved the slot and drew frames"),
    );
    ctx.ok(
        (cols_back - cols_open).abs() < 0.5,
        format!(
            "sidebar {pct}-{renderer}: the terminal is as wide after a round trip ({cols_open} -> {cols_back} columns)"
        ),
    );

    // The pictures: one snapshot per offset into each slide.
    let mut shots = Vec::new();
    for (name, to_open) in [("collapse", false), ("expand", true)] {
        if to_open {
            // The expand starts collapsed.
            toggle(ctx, &mut app, false)?;
        }
        for (k, off) in OFFSETS.iter().enumerate() {
            let t0 = Instant::now();
            app.session.key("Ctrl+Shift+B")?;
            wait_until(t0 + Duration::from_millis(*off));
            let at = t0.elapsed().as_secs_f64() * 1000.0;
            // The picture comes first; its label is the slide the app read straight after taking it, so
            // the number names the frame the picture shows (a probe before the snapshot is a frame early).
            let staged = dir
                .join("frames")
                .join(format!("{name}-{k:02}-t{:03}ms.png", at.round() as i64));
            let taken = shot_to(&mut app, &staged);
            let shown = picture_slide(&mut app);
            let slot = shown.map_or(0, |s| s.0.round() as i64);
            let file = dir.join("frames").join(format!(
                "{name}-{k:02}-t{:03}ms-slot{slot:03}.png",
                at.round() as i64
            ));
            let taken = taken && fs::rename(&staged, &file).is_ok();
            shots.push(json!({
                "name": name, "k": k, "target_ms": off, "at_ms": at,
                "slot": shown.map(|s| s.0), "panel_left": shown.map(|s| s.1), "taken": taken,
                "file": file.file_name().map(|f| f.to_string_lossy()),
            }));
            // Back to the state this pass started from, settled.
            pause(520);
            app.session.key("Ctrl+Shift+B")?;
            pause(520);
        }
    }
    let taken = shots.iter().filter(|s| s["taken"] == json!(true)).count();
    ctx.ok(
        taken == OFFSETS.len() * 2,
        format!(
            "sidebar {pct}-{renderer}: {taken} snapshots taken during the slides (of {})",
            OFFSETS.len() * 2
        ),
    );
    let problems = park_problems(&probe(&mut app, "window.park", Value::Null));
    ctx.ok(
        problems.is_empty(),
        format!("sidebar {pct}-{renderer}: the window stayed parked ({problems:?})"),
    );
    let summary = json!({
        "surface": "sidebar", "pct": pct, "renderer": renderer,
        "hello": { "renderer": hello.renderer, "scale": hello.scale, "version": hello.version },
        "window_css": [SIZE.0, SIZE.1],
        "cols": { "open": cols_open, "collapsed": cols_collapsed, "back": cols_back },
        "collapse": collapse, "expand": expand,
        "medians": {
            "collapse_slot": { "longest_gap_ms": median_of(&field(&collapse, "slot"), "longest_gap_ms"),
                "frames": median_of(&field(&collapse, "slot"), "frames"),
                "t99_ms": median_of(&field(&collapse, "slot"), "t99_ms"),
                "settle_ms": median_of(&field(&collapse, "slot"), "settle_ms"),
                "dev_best": median_of(&field(&collapse, "slot"), "dev_best") },
            "expand_slot": { "longest_gap_ms": median_of(&field(&expand, "slot"), "longest_gap_ms"),
                "frames": median_of(&field(&expand, "slot"), "frames"),
                "t99_ms": median_of(&field(&expand, "slot"), "t99_ms"),
                "settle_ms": median_of(&field(&expand, "slot"), "settle_ms"),
                "dev_best": median_of(&field(&expand, "slot"), "dev_best") },
        },
        "snapshots": shots,
    });
    save_numbers(&dir, &summary)?;
    app.close();
    Ok(())
}

fn field(runs: &[Value], key: &str) -> Vec<Value> {
    runs.iter().map(|r| r[key].clone()).collect()
}

fn startup_cell(
    ctx: &mut Ctx,
    cfg: &Cfg,
    renderer: &'static str,
    surface: &'static str,
) -> Result<(), String> {
    // The native scale: the app starts at the monitor's and the tour does not change it for this surface.
    let w = ctx.new_world();
    let g = gamma(&w);
    let mut settings = vec![("flare.term.shell", "cmd")];
    if surface == "resume" {
        settings.push(("flare.window.tabLayout", "vertical"));
        settings.push(("flare.window.sidebar", "open"));
    }
    seed(&w.profile, &settings)?;
    let (folders, want_tabs) = if surface == "resume" {
        seed_tabs(&w.profile, &[&w.alpha, &w.beta, &g], 1)?;
        (Vec::new(), 3)
    } else {
        (vec![w.alpha.clone()], 1)
    };
    wake_display();
    // The frames and the app's own numbers from one watched launch.
    let probe_dir = cfg.out.join(surface).join("_scratch");
    let (watch, mut app) = match watch_launch(
        ctx,
        &w,
        renderer,
        &folders,
        want_tabs,
        &probe_dir.join(renderer),
    ) {
        Ok(x) => x,
        Err(e) => {
            ctx.skip(&format!("{surface} {renderer}: {e}"));
            return Ok(());
        }
    };
    let hello = app.hello()?;
    let pct = (f64::from(hello.scale) * 100.0).round() as u32;
    let dir = cfg.dir(surface, pct, renderer);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let _ = fs::rename(probe_dir.join(renderer), dir.join("frames"));
    let _ = fs::remove_dir(&probe_dir);
    if renderer == "gpu" && !hello.renderer.to_lowercase().starts_with("gpu") {
        ctx.skip(&format!(
            "{surface} gpu: no GPU adapter ({})",
            hello.renderer
        ));
        app.close();
        return Ok(());
    }
    let front = Tabs::read(&mut app).and_then(|t| t.front_index());
    let judged = judge_frames(&watch.frames);
    // The copy must be the window's own size in device pixels (audit V2: a copy taken with a
    // DPI-unaware process came back a crop of the window with a black block where it did not reach).
    let rect = probe(&mut app, "window.park", Value::Null)["rect"].clone();
    let own = (
        (num(&rect[2]) - num(&rect[0])) as u32,
        (num(&rect[3]) - num(&rect[1])) as u32,
    );
    let copied = watch
        .frames
        .last()
        .map_or((0, 0), |f| (num(&f["w"]) as u32, num(&f["h"]) as u32));
    ctx.ok(
        own.0 > 0 && copied == own,
        format!(
            "{surface} {renderer}: the copy is the window's own size {own:?} (copied {copied:?})"
        ),
    );
    // The last picture is the dark ground with ink on it, never a window-sized block of pure black
    // (audit V2): its mean colour over the sample is well above black.
    let mean = watch.frames.last().map_or(vec![0.0; 3], |f| {
        f["mean"]
            .as_array()
            .map(|a| a.iter().map(num).collect())
            .unwrap_or_default()
    });
    ctx.ok(
        mean.iter().all(|c| *c > 10.0),
        format!(
            "{surface} {renderer}: the last picture is painted, not black (mean colour {mean:?})"
        ),
    );
    // What the screen can show (audit V1): from the moment the window is shown and not cloaked, every
    // picture is the app's own (not black), frameless, at the window's size, and the first is painted.
    // The pictures copied before then are named `-hidden` and DWM never composed them.
    let seen = |k: &str| judged[k].as_u64().unwrap_or(u64::MAX);
    ctx.ok(
        watch.first_visible_ms.is_some() && seen("seen_frames") >= 1,
        format!(
            "{surface} {pct}-{renderer}: the window came on the screen at {:?} ms ({} picture(s) before, not shown)",
            watch.first_visible_ms,
            judged["hidden_frames"]
        ),
    );
    ctx.ok(
        seen("seen_blank_frames") == 0 && judged["first_seen_painted"] == json!(true),
        format!(
            "{surface} {pct}-{renderer}: no black picture once on the screen; the first is painted ({} blank)",
            judged["seen_blank_frames"]
        ),
    );
    ctx.ok(
        seen("seen_framed_frames") == 0,
        format!(
            "{surface} {pct}-{renderer}: frameless from the first picture on the screen ({} framed)",
            judged["seen_framed_frames"]
        ),
    );
    ctx.ok(
        seen("seen_wrong_size_frames") == 0 && judged["first_seen_size"] == json!([own.0, own.1]),
        format!(
            "{surface} {pct}-{renderer}: the first picture on the screen is already the window's size {own:?} (first {}, {} other size)",
            judged["first_seen_size"], judged["seen_wrong_size_frames"]
        ),
    );
    let series_rows = slide::parse_rows(&watch.series);
    let first_frames: Vec<Value> = series_rows
        .iter()
        .take(40)
        .map(|r| json!([r.age, r.gap, r.render]))
        .collect();
    let first_prompt = watch.trace["phases"]
        .as_array()
        .and_then(|p| p.iter().find(|x| x["name"] == "first_prompt"))
        .map_or(0.0, |x| num(&x["ms"]));
    let longest_early_gap = series_rows
        .iter()
        .take_while(|r| r.age < first_prompt + 1500.0)
        .filter_map(|r| r.gap)
        .fold(0.0, f64::max);
    ctx.ok(
        watch.frames.len() >= 2,
        format!(
            "{surface} {pct}-{renderer}: {} different pictures were copied from the window",
            watch.frames.len()
        ),
    );
    ctx.ok(
        watch.ready_ms.is_some(),
        format!("{surface} {pct}-{renderer}: {want_tabs} tab(s) and a prompt came up"),
    );
    if surface == "resume" {
        ctx.ok(
            front == Some(1),
            format!("resume {pct}-{renderer}: the saved middle tab is in front (index {front:?})"),
        );
        // A tab that popped in, changed front or changed folder after the first time all three were there.
        let all_three = watch.timeline.iter().position(|t| t["tabs"] == json!(3));
        let churn = all_three.map_or(0, |i| watch.timeline.len() - 1 - i);
        ctx.ok(
            churn <= 1,
            format!("resume {pct}-{renderer}: after the tabs were all there the strip changed {churn} more time(s)"),
        );
    }
    let out = json!({
        "surface": surface, "pct": pct, "renderer": renderer,
        "hello": { "renderer": hello.renderer, "scale": hello.scale, "version": hello.version },
        "first_window_ms": watch.first_window_ms, "first_visible_ms": watch.first_visible_ms,
        "first_pipe_ms": watch.first_pipe_ms,
        "ready_ms": watch.ready_ms,
        "judged": judged,
        "app_trace": watch.trace,
        "longest_frame_gap_ms_to_1500_after_first_prompt": longest_early_gap,
        "first_presented_frames": first_frames,
        "timeline": watch.timeline,
        "frames": watch.frames,
    });
    save_numbers(&dir, &out)?;
    app.close();
    Ok(())
}

/// Start-up timing: median of launches without the PrintWindow observer.
fn startup_timing(
    ctx: &mut Ctx,
    cfg: &Cfg,
    renderer: &'static str,
    surface: &'static str,
) -> Result<Value, String> {
    let mut runs = Vec::new();
    for i in 0..=cfg.runs {
        let w = ctx.new_world();
        let g = gamma(&w);
        let mut settings = vec![("flare.term.shell", "cmd")];
        if surface == "resume" {
            settings.push(("flare.window.tabLayout", "vertical"));
            settings.push(("flare.window.sidebar", "open"));
        }
        seed(&w.profile, &settings)?;
        let (folders, want) = if surface == "resume" {
            seed_tabs(&w.profile, &[&w.alpha, &w.beta, &g], 1)?;
            (Vec::new(), 3)
        } else {
            (vec![w.alpha.clone()], 1)
        };
        let mut args: Vec<String> =
            vec!["--no-import".into(), "--renderer".into(), renderer.into()];
        args.extend(folders.iter().map(|f| f.to_string_lossy().into_owned()));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let began = Instant::now();
        let child = Child::launch(&ctx.exe, &refs, &w.env(), &w.profile);
        let session = Session::connect(&w.profile, Duration::from_secs(20))?;
        let mut app = App {
            child,
            session,
            profile: w.profile.clone(),
        };
        let pipe_ms = began.elapsed().as_secs_f64() * 1000.0;
        let mut tabs_ms = None;
        let mut prompt_ms = None;
        let end = Instant::now() + Duration::from_secs(30);
        while Instant::now() < end && prompt_ms.is_none() {
            if let Some(st) = Tabs::read(&mut app) {
                if st.len() >= want && tabs_ms.is_none() {
                    tabs_ms = Some(began.elapsed().as_secs_f64() * 1000.0);
                }
                if st.len() >= want {
                    let id = st.front.clone().unwrap_or_default();
                    if prompt_showing(&rows_text(&mut app, &id), Shell::Cmd) {
                        prompt_ms = Some(began.elapsed().as_secs_f64() * 1000.0);
                    }
                }
            }
            pause(10);
        }
        let trace = app
            .session
            .probe_str("startup.trace")
            .unwrap_or(Value::Null);
        app.close();
        if i == 0 {
            continue; // the first launch warms the disk cache and is thrown away
        }
        let phase = |name: &str| -> Option<f64> {
            trace["phases"]
                .as_array()?
                .iter()
                .find(|p| p["name"] == name)?["ms"]
                .as_f64()
        };
        runs.push(json!({
            "pipe_ms": pipe_ms, "tabs_ms": tabs_ms, "prompt_wall_ms": prompt_ms,
            "main": phase("main"), "component": phase("component"), "shell_spawn": phase("shell_spawn"),
            "render_setup": phase("render_setup"), "first_frame": phase("first_frame"),
            "first_prompt": phase("first_prompt"),
        }));
    }
    let med = |k: &str| {
        median(
            &runs
                .iter()
                .filter_map(|r| r[k].as_f64())
                .collect::<Vec<_>>(),
        )
    };
    Ok(json!({
        "runs": runs,
        "median": {
            "pipe_ms": med("pipe_ms"), "tabs_ms": med("tabs_ms"), "prompt_wall_ms": med("prompt_wall_ms"),
            "main": med("main"), "component": med("component"), "shell_spawn": med("shell_spawn"),
            "render_setup": med("render_setup"), "first_frame": med("first_frame"), "first_prompt": med("first_prompt"),
        },
    }))
}

// ---- the general pass ---------------------------------------------------------------------------------

fn named(dir: &Path, n: &mut u32, name: &str) -> PathBuf {
    *n += 1;
    dir.join("frames").join(format!("{:02}-{name}.png", *n))
}

fn general_cell(
    ctx: &mut Ctx,
    cfg: &Cfg,
    pct: u32,
    dpi: u32,
    renderer: &'static str,
) -> Result<(), String> {
    let dir = cfg.dir("general", pct, renderer);
    let _ = fs::remove_dir_all(dir.join("frames"));
    let mut n = 0u32;
    let mut notes: Vec<Value> = Vec::new();
    let w = ctx.new_world();
    seed(&w.profile, &[("flare.term.shell", "cmd")])?;
    let g = gamma(&w);
    let mut app = start(
        ctx,
        &w,
        renderer,
        Some(dpi),
        vec![w.alpha.clone(), w.beta.clone(), g],
        Vec::new(),
    )?;
    let label = format!("general {pct}-{renderer}");
    let ok = ready(ctx, &mut app, 3);
    ctx.ok(ok, format!("{label}: three tabs and a prompt"));
    if !ok {
        app.close();
        return Ok(());
    }
    let take = |ctx: &mut Ctx, app: &mut App, n: &mut u32, name: &str, notes: &mut Vec<Value>| {
        let p = named(&dir, n, name);
        let t = shot_to(app, &p);
        notes.push(json!({ "name": name, "taken": t }));
        ctx.ok(t, format!("{label}: a picture of {name}"));
    };
    take(ctx, &mut app, &mut n, "tabs-strip", &mut notes);

    // The tab menu: a right click on the first tab.
    let tabs = Tabs::read(&mut app);
    if let Some((x, y)) = tabs.as_ref().and_then(|t| t.centre(0)) {
        // A pointer is on a whole device pixel. The tab's centre can be a half pixel (17.5), which
        // the menu's device-pixel snap rounds to the row below; Prism Terminal's click truncates.
        let _ = app.session.right_click(x.floor(), y.floor());
        let open = ctx.until("the tab menu", 3_000, &mut || {
            probe(&mut app, "app.state", Value::Null)["menu"]["open"] == json!(true)
        });
        pause(250);
        take(ctx, &mut app, &mut n, "tab-menu", &mut notes);
        ctx.ok(open, format!("{label}: the tab menu opened"));
        let _ = app.session.key("Escape");
        pause(200);
    }

    // The command help.
    let _ = app.session.key("F1");
    let help = ctx.until("the help panel", 6_000, &mut || {
        probe(&mut app, "help.state", Value::Null)["open"] == json!(true)
    });
    ctx.until("the help panel to finish sliding", 8_000, &mut || {
        probe(&mut app, "help.state", Value::Null)["slid"] == json!(true)
    });
    pause(300);
    take(ctx, &mut app, &mut n, "help-panel", &mut notes);
    ctx.ok(help, format!("{label}: the help panel opened"));
    let _ = app.session.key("Escape");
    pause(400);

    // Settings and two pages.
    let opened = driver::open_settings(ctx, &mut app);
    ctx.ok(opened, format!("{label}: Settings opened"));
    if opened {
        pause(300);
        take(ctx, &mut app, &mut n, "settings-first-page", &mut notes);
        for id in ["appearance", "dictation", "about"] {
            let r = if id == "about" {
                probe(&mut app, "settings.rail_end", Value::Null)
            } else {
                probe(&mut app, "settings.rail", json!({ "id": id }))
            };
            if let Some((x, y)) = rect_c(&r) {
                let _ = app.session.click(x, y);
                let there = ctx.until(&format!("the {id} page"), 6_000, &mut || {
                    probe(&mut app, "settings.rows", json!({}))["page"].as_str() == Some(id)
                });
                pause(300);
                take(ctx, &mut app, &mut n, &format!("settings-{id}"), &mut notes);
                ctx.ok(there, format!("{label}: the {id} page is drawn"));
            }
        }
        // A theme switch: the wall pick, a frame at once and one settled.
        let appear = probe(&mut app, "settings.rail", json!({ "id": "appearance" }));
        if let Some((x, y)) = rect_c(&appear) {
            let _ = app.session.click(x, y);
            pause(300);
        }
        for theme in ["nord", "paper", "pt-default"] {
            let picked = driver::pick_theme(ctx, &mut app, theme);
            let p0 = named(&dir, &mut n, &format!("theme-{theme}-at-once"));
            let t0 = shot_to(&mut app, &p0);
            pause(450);
            take(
                ctx,
                &mut app,
                &mut n,
                &format!("theme-{theme}-settled"),
                &mut notes,
            );
            notes.push(
                json!({ "name": format!("theme-{theme}-at-once"), "taken": t0, "picked": picked }),
            );
            ctx.ok(picked, format!("{label}: the {theme} theme was picked"));
        }
        // Back to a terminal.
        let shell = Tabs::read(&mut app).and_then(|t| {
            let i = t.list.iter().position(|x| x["kind"] == json!("shell"))?;
            t.centre(i)
        });
        if let Some((x, y)) = shell {
            let _ = app.session.click(x, y);
            pause(400);
        }
    }
    let problems = park_problems(&probe(&mut app, "window.park", Value::Null));
    ctx.ok(
        problems.is_empty(),
        format!("{label}: the window stayed parked ({problems:?})"),
    );
    app.close();

    // See-through: a second launch with the level stored.
    let w2 = ctx.new_world();
    seed(
        &w2.profile,
        &[("flare.term.shell", "cmd"), ("flare.term.seeThrough", "40")],
    )?;
    // Prism Terminal's see-through frame is taken in its three-tab session, so this one has the same
    // three tabs (alpha, beta, gamma), not one.
    let g2 = gamma(&w2);
    let mut app = start(
        ctx,
        &w2,
        renderer,
        Some(dpi),
        vec![w2.alpha.clone(), w2.beta.clone(), g2],
        Vec::new(),
    )?;
    let ok = ready(ctx, &mut app, 3);
    ctx.ok(ok, format!("{label}: see-through 40 launched"));
    if ok {
        take(ctx, &mut app, &mut n, "see-through-40", &mut notes);
    }
    app.close();

    save_numbers(
        &dir,
        &json!({ "surface": "general", "pct": pct, "renderer": renderer, "pictures": notes }),
    )?;
    Ok(())
}

// ---- the dictation pill --------------------------------------------------------------------------------

fn dictation_cell(
    ctx: &mut Ctx,
    cfg: &Cfg,
    pct: u32,
    dpi: u32,
    renderer: &'static str,
) -> Result<(), String> {
    let label = format!("dictation {pct}-{renderer}");
    let dir = cfg.dir("dictation", pct, renderer);
    let _ = fs::remove_dir_all(dir.join("frames"));
    let model = match crate::fixtures::require("ggml-tiny.bin") {
        Ok(m) => m,
        Err(e) => {
            ctx.skip(&format!("{label}: {e}"));
            return Ok(());
        }
    };
    let clip = crate::fixtures::require("jfk.wav")?;
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest.parent().and_then(Path::parent).unwrap_or(manifest);
    let engine = repo.join("vendor").join("whisper");
    let w = ctx.new_world();
    let root = w.profile.join("dictation");
    let models = root.join("models");
    fs::create_dir_all(&models).map_err(|e| e.to_string())?;
    if fs::hard_link(&model, models.join("tiny.bin")).is_err() {
        fs::copy(&model, models.join("tiny.bin")).map_err(|e| e.to_string())?;
    }
    seed(
        &w.profile,
        &[
            ("flare.term.shell", "cmd"),
            (
                "flare.dictation.hotkey",
                r#"{"code":"AltRight","ctrl":false,"alt":false,"shift":false,"meta":false}"#,
            ),
            ("flare.dictation.model", "tiny"),
            ("flare.dictation.enabled", "1"),
            ("flare.dictation.sounds", "0"),
            ("flare.dictation.pauseMedia", "0"),
        ],
    )?;
    let env = vec![
        (
            concat!("FLARE_E2E", "_MIC").to_string(),
            clip.to_string_lossy().into_owned(),
        ),
        (
            concat!("FLARE_DICTATION", "_ROOT").to_string(),
            root.to_string_lossy().into_owned(),
        ),
        (concat!("FLARE_E2E", "_NVIDIA").to_string(), "0".to_string()),
        (
            concat!("FLARE_WHISPER", "_DIR").to_string(),
            engine.to_string_lossy().into_owned(),
        ),
    ];
    let mut app = start(ctx, &w, renderer, Some(dpi), vec![w.alpha.clone()], env)?;
    let ok = ready(ctx, &mut app, 1);
    ctx.ok(ok, format!("{label}: a tab and a prompt"));
    if !ok {
        app.close();
        return Ok(());
    }
    // The engine is warmed a moment after it is switched on.
    pause(3_000);
    if let Some((x, y)) = rect_c(&probe(&mut app, "ui.rect", json!({ "name": "grid" }))) {
        let _ = app.session.click(x, y);
    }
    let _ = app.session.call(&Request::Key {
        chord: "AltRight".into(),
        down: true,
    });
    let listening = ctx.until("the pill to show Listening", 15_000, &mut || {
        let p = probe(&mut app, "dict.pill", Value::Null);
        p["mounted"] == json!(true) && p["face"] == json!(1)
    });
    ctx.ok(listening, format!("{label}: the pill shows Listening"));
    let mut notes = Vec::new();
    let mut n = 0u32;
    if listening {
        pause(600);
        let p = named(&dir, &mut n, "pill-listening");
        notes.push(json!({ "name": "pill-listening", "taken": shot_to(&mut app, &p) }));
        let live = ctx.until("live text", 15_000, &mut || {
            probe(&mut app, "dict.view", Value::Null)["live_text"]
                .as_str()
                .is_some_and(|t| !t.trim().is_empty())
        });
        pause(500);
        let p = named(
            &dir,
            &mut n,
            if live {
                "pill-live-text"
            } else {
                "pill-no-live-text"
            },
        );
        notes.push(
            json!({ "name": "pill-live-text", "taken": shot_to(&mut app, &p), "live": live }),
        );
    }
    let _ = app.session.call(&Request::Key {
        chord: "AltRight".into(),
        down: false,
    });
    let transcribing = ctx.poll(4_000, &mut || {
        probe(&mut app, "dict.view", Value::Null)["transcribing"] == json!(true)
    });
    if transcribing {
        let p = named(&dir, &mut n, "pill-transcribing");
        notes.push(json!({ "name": "pill-transcribing", "taken": shot_to(&mut app, &p) }));
    }
    ctx.until("the pill to go", 40_000, &mut || {
        probe(&mut app, "dict.view", Value::Null)["active"] != json!(true)
    });
    let widget = probe(&mut app, dict_probe_widget(), Value::Null);
    notes.push(json!({ "widget_probe": widget }));
    save_numbers(
        &dir,
        &json!({ "surface": "dictation", "pct": pct, "renderer": renderer, "pictures": notes }),
    )?;
    app.close();
    Ok(())
}

fn dict_probe_widget() -> &'static str {
    flare_e2e_proto::dict_probes::WIDGET
}

/// One surface of one cell of the matrix.
type CellRun = fn(&mut Ctx, &Cfg, u32, u32, &'static str) -> Result<(), String>;

// ---- the scenario -----------------------------------------------------------------------------------------

/// The tour. One scenario, filtered by the environment (see the module doc).
pub fn visual_tour(ctx: &mut Ctx) -> Result<(), String> {
    let cfg = cfg();
    ctx.ok(
        !cfg.cells.is_empty() && !cfg.surfaces.is_empty(),
        format!(
            "the tour has cells {:?} and surfaces {:?}",
            cfg.cells
                .iter()
                .map(|c| format!("{}-{}", c.0, c.2))
                .collect::<Vec<_>>(),
            cfg.surfaces
        ),
    );
    wake_display();
    let mut timings = serde_json::Map::new();
    for surface in ["startup", "resume"] {
        if !cfg.wants(surface) {
            continue;
        }
        for renderer in RENDERERS {
            if cfg.cells.iter().all(|c| c.2 != renderer) {
                continue;
            }
            let label = format!("{surface} {renderer}");
            match startup_timing(
                ctx,
                &cfg,
                renderer,
                if surface == "startup" {
                    "startup"
                } else {
                    "resume"
                },
            ) {
                Ok(t) => {
                    ctx.ok(true, format!("{label}: {} timed launches", cfg.runs));
                    timings.insert(format!("{surface}-{renderer}"), t);
                }
                Err(e) => ctx.skip(&format!("{label} timing: {e}")),
            }
            if let Err(e) = startup_cell(
                ctx,
                &cfg,
                renderer,
                if surface == "startup" {
                    "startup"
                } else {
                    "resume"
                },
            ) {
                ctx.ok(false, format!("{label}: {e}"));
            }
        }
    }
    if !timings.is_empty() {
        let f = cfg
            .out
            .join(format!("timings-flare-{}.json", std::process::id()));
        let _ = fs::create_dir_all(&cfg.out);
        let _ = fs::write(
            &f,
            serde_json::to_string_pretty(&Value::Object(timings)).unwrap_or_default(),
        );
    }
    for (pct, dpi, renderer) in cfg.cells.clone() {
        let cell: [(&str, CellRun); 3] = [
            ("sidebar", sidebar_cell),
            ("general", general_cell),
            ("dictation", dictation_cell),
        ];
        for (surface, run) in cell {
            if !cfg.wants(surface) {
                continue;
            }
            if let Err(e) = run(ctx, &cfg, pct, dpi, renderer) {
                if e.starts_with("no GPU") {
                    ctx.skip(&format!("{surface} {pct}-{renderer}: {e}"));
                } else {
                    ctx.ok(false, format!("{surface} {pct}-{renderer}: {e}"));
                }
            }
        }
    }
    ctx.finish();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_filter_is_none_for_all_or_nothing() {
        assert_eq!(list_filter(None), None);
        assert_eq!(list_filter(Some("")), None);
        assert_eq!(list_filter(Some("ALL")), None);
        assert_eq!(
            list_filter(Some("100-gpu, 225-software")),
            Some(vec!["100-gpu".to_string(), "225-software".to_string()])
        );
    }

    #[test]
    fn the_cell_directory_is_surface_then_scale_and_renderer() {
        let c = Cfg {
            out: PathBuf::from("o"),
            cells: Vec::new(),
            surfaces: vec!["sidebar".into()],
            runs: 1,
        };
        assert_eq!(
            c.dir("sidebar", 225, "gpu"),
            PathBuf::from("o").join("sidebar").join("225-gpu")
        );
        assert!(c.wants("sidebar") && !c.wants("general"));
    }
}
