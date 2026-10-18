//! Core probes: app state, the grid, geometry, the clipboard, the openers, the start-up trace and
//! the DPI numbers (Task 11). Every probe only looks: nothing here borrows the controller mutably
//! or sets a property (a scan test holds that).
//!
//! Geometry is read from what the window exports (`grid-left`, `tip-x`, `strip-phys-width`, the
//! tab rows) and from the title bar's own constants; a rectangle the layout does not export yet is
//! an error that says so, never a guess.

use flare_render::color::{Palette, resolve_bg, resolve_fg};
use flare_render::grid::CellFlags;
use serde_json::{Value, json};
use slint::{ComponentHandle, Model};

use super::{ProbeCtx, ProbeRegistry};

/// The title bar's height in css pixels (`Theme.bar-height`).
const BAR_H: f32 = 36.0;
/// A title-bar button: 32 wide, 28 high, 4 apart, 6 from the window's right edge.
const BTN_W: f32 = 32.0;
const BTN_H: f32 = 28.0;
const BTN_GAP: f32 = 4.0;
const BTN_RIGHT: f32 = 6.0;

/// Adds this file's probes to `r`.
pub fn register(r: &mut ProbeRegistry) {
    r.add("app.state", app_state);
    r.add("grid.text", grid_text);
    r.add("grid.cell", grid_cell);
    r.add("ui.rect", ui_rect);
    r.add("clip.last", clip_last);
    r.add("open.log", open_log);
    r.add("flash.count", flash_count);
    r.add("startup.trace", startup_trace);
    r.add("dpi.state", dpi_state);
    r.add("probe.names", probe_names);
}

fn hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn scale_of(ctx: &ProbeCtx) -> f32 {
    ctx.app.window().scale_factor()
}

fn css_size(ctx: &ProbeCtx) -> (f32, f32) {
    let s = ctx.app.window().size();
    let k = scale_of(ctx);
    (s.width as f32 / k, s.height as f32 / k)
}

/// One rectangle in css pixels and in device pixels (css times the scale, rounded).
fn rect_json(x: f32, y: f32, w: f32, h: f32, scale: f32) -> Value {
    let d = |v: f32| (v * scale).round() as i64;
    json!({
        "x": x, "y": y, "w": w, "h": h,
        "dx": d(x), "dy": d(y), "dw": d(w), "dh": d(h),
    })
}

fn mark_state_name(n: i32) -> &'static str {
    match n {
        1 => "working",
        2 => "question",
        3 => "finished",
        4 => "failed",
        _ => "none",
    }
}

fn mark_motion_name(n: i32) -> &'static str {
    match n {
        1 => "ring",
        2 => "line",
        3 => "run",
        _ => "none",
    }
}

fn kind_name(row: &crate::TabRow) -> &'static str {
    if row.settings {
        "settings"
    } else {
        match row.agent {
            1 => "claude",
            2 => "codex",
            _ => "shell",
        }
    }
}

fn rows_of(ctx: &ProbeCtx) -> Vec<crate::TabRow> {
    ctx.app.get_tab_rows().iter().collect()
}

/// The hairline in css pixels: max(1, floor(scale)) device pixels (`Px.hair()`).
fn hair(k: f32) -> f32 {
    k.floor().max(1.0) / k
}

/// `Px.snap(l)`: a length rounded to whole device pixels, in css pixels.
fn snap(l: f32, k: f32) -> f32 {
    (l * k).round() / k
}

/// The title mark before the strip (`Px.snap(42px) + Px.hair()` wide); the strip starts after it.
fn mark_w(k: f32) -> f32 {
    snap(42.0, k) + hair(k)
}

/// Where a tab is in the strip (css px). Only the horizontal strip exports it.
fn tab_rect(ctx: &ProbeCtx, row: &crate::TabRow) -> Value {
    if ctx.app.get_layout_vertical() {
        return Value::Null;
    }
    let k = scale_of(ctx);
    rect_json(
        mark_w(k) + row.x as f32 / k,
        0.0,
        row.w as f32 / k,
        BAR_H - hair(k),
        k,
    )
}

fn app_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let app = ctx.app;
    let k = scale_of(ctx);
    // The window glue's word when it gave one ("gpu" or "software (fallback: <why>)"), else the
    // renderer the run asked for.
    let (told, _) = crate::e2e_hook::hello_info();
    let told = told.to_lowercase();
    let (renderer, fallback) = if told.is_empty() {
        let asked = match ctx.args.renderer {
            crate::config::RendererChoice::Gpu => "gpu",
            crate::config::RendererChoice::Software => "software",
        };
        (asked, None)
    } else {
        let why = told
            .split_once("fallback:")
            .map(|(_, r)| r.trim().trim_end_matches(')').to_string());
        (
            if told.starts_with("gpu") {
                "gpu"
            } else {
                "software"
            },
            why,
        )
    };
    let win = app.window();
    let pos = win.position();
    let size = win.size();

    let (front, tabs_cwd, settings_open, settings_front) = {
        let c = ctx.ctl.try_borrow().map_err(|_| "the controller is busy")?;
        let st = &c.model().state;
        (
            st.active_id.clone(),
            st.tabs
                .iter()
                .map(|t| (t.id.clone(), t.cwd.clone()))
                .collect::<Vec<_>>(),
            st.tabs
                .iter()
                .any(|t| t.kind == flare_tabs::tabs::TabKind::Settings),
            st.active_id.as_deref() == Some(flare_tabs::tabs::SETTINGS_ID),
        )
    };
    let tabs: Vec<Value> = rows_of(ctx)
        .iter()
        .map(|row| {
            let id = row.id.to_string();
            let cwd = tabs_cwd
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, c)| c.clone())
                .unwrap_or_default();
            json!({
                "id": id,
                "title": row.label.to_string(),
                "tooltip": row.tip.to_string(),
                "cwd": cwd,
                "front": row.active,
                "kind": kind_name(row),
                "mark": {
                    "state": mark_state_name(row.mark_state),
                    "motion": mark_motion_name(row.mark_kind),
                    "colour": hex(row.mark_color.red(), row.mark_color.green(), row.mark_color.blue()),
                },
                "rect": tab_rect(ctx, row),
            })
        })
        .collect();

    let menu_rows: Vec<Value> = app
        .get_menu_rows()
        .iter()
        .map(|r| {
            json!({
                "label": r.label.to_string(),
                "hint": r.hint.to_string(),
                "icon": r.icon.to_string(),
                "disabled": r.disabled,
            })
        })
        .collect();

    let dialog_open = app.get_dialog_open();
    let dialog = json!({
        "open": dialog_open,
        "title": if dialog_open { Value::from(app.get_dialog_title().to_string()) } else { Value::Null },
        "body": if dialog_open { Value::from(format!("{:?}", app.get_dialog_body())) } else { Value::Null },
        "buttons": if dialog_open {
            json!(["Cancel", app.get_dialog_primary().to_string()])
        } else {
            json!([])
        },
        "focus": Value::Null,
    });

    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "renderer": renderer,
        "fallback_reason": fallback,
        "scale": k,
        "window": {
            "x": pos.x, "y": pos.y, "w": size.width, "h": size.height,
            "client_w": size.width, "client_h": size.height,
        },
        "front": front,
        "tabs": tabs,
        "sidebar": {
            "open": app.get_layout_vertical() && !app.get_sidebar_collapsed(),
            "collapsed": app.get_sidebar_collapsed(),
            "w": app.get_sidebar_width(),
        },
        "dialog": dialog,
        "menu": { "open": app.get_menu_open(), "rows": menu_rows },
        "find": {
            "open": app.get_find_open(),
            "query": app.get_find_query().to_string(),
            "readout": app.get_find_readout().to_string(),
        },
        "badge": {
            "text": "Copied",
            "visible": app.get_copied_phase() != 0,
        },
        "tip": {
            "text": app.get_tip_text().to_string(),
            "visible": app.get_tip_visible(),
            "rect": if app.get_tip_visible() { tip_rect(ctx) } else { Value::Null },
        },
        "settings": {
            "open": settings_open,
            "page": Value::Null,
            "front": settings_front,
        },
        "quitting": !win.is_visible(),
    }))
}

fn tip_rect(ctx: &ProbeCtx) -> Value {
    let a = ctx.app;
    rect_json(
        a.get_tip_x(),
        a.get_tip_y(),
        a.get_tip_w(),
        a.get_tip_h(),
        scale_of(ctx),
    )
}

/// The shell tab a grid probe reads: the argument's `tab`, else the one in front.
fn shell_of(
    ctx: &ProbeCtx,
    arg: &Value,
) -> Result<(String, std::sync::Arc<flare_term::handle::TermHandle>), String> {
    let front = ctx
        .ctl
        .try_borrow()
        .map_err(|_| "the controller is busy")?
        .model()
        .state
        .active_id
        .clone()
        .ok_or("no tab is in front")?;
    if let Some(t) = arg.get("tab").and_then(Value::as_str)
        && t != front
    {
        return Err(format!(
            "tab {t} is not in front: only the front tab's grid can be read for now"
        ));
    }
    let h = ctx
        .front
        .clone()
        .ok_or_else(|| format!("tab {front} has no shell"))?;
    Ok((front, h))
}
fn grid_text(ctx: &ProbeCtx, arg: &Value) -> Result<Value, String> {
    let (id, h) = shell_of(ctx, arg)?;
    let title = rows_of(ctx)
        .iter()
        .find(|r| r.id == id.as_str())
        .map(|r| r.label.to_string())
        .unwrap_or_default();
    Ok(h.with_emu_quiet(|e| {
        let rows: Vec<String> = (0..e.rows() as i32).map(|l| e.row_text(l)).collect();
        let (row, col) = e.cursor();
        let visible = e.mode().cursor_visible;
        json!({
            "rows": rows,
            "cols": e.cols(),
            "rows_n": e.rows(),
            "cursor": { "row": row, "col": col, "visible": visible },
            "alt_screen": e.mode().alt_screen,
            "history": e.history_size(),
            "title": title,
        })
    }))
}

fn grid_cell(ctx: &ProbeCtx, arg: &Value) -> Result<Value, String> {
    let row = arg
        .get("row")
        .and_then(Value::as_u64)
        .ok_or("grid.cell needs a row")? as usize;
    let col = arg
        .get("col")
        .and_then(Value::as_u64)
        .ok_or("grid.cell needs a col")? as usize;
    let (_, h) = shell_of(ctx, arg)?;
    let grid = h.with_emu_quiet(|e| e.snapshot());
    if row >= grid.rows() || col >= grid.cols() {
        return Err(format!(
            "cell ({row},{col}) is outside the {} by {} grid",
            grid.rows(),
            grid.cols()
        ));
    }
    let cell = grid.cell(col, row);
    // The colours of the default palette: a theme change moves the defaults, not an explicit RGB.
    let pal = Palette::pt_default();
    let f = cell.flags;
    let fg = resolve_fg(&pal, cell.fg, f.contains(CellFlags::BOLD));
    let bg = resolve_bg(&pal, cell.bg);
    Ok(json!({
        "ch": cell.cluster(),
        "fg": hex(fg.r, fg.g, fg.b),
        "bg": hex(bg.r, bg.g, bg.b),
        "bold": f.contains(CellFlags::BOLD),
        "italic": f.contains(CellFlags::ITALIC),
        "underline": f.contains(CellFlags::UNDERLINE),
        "inverse": f.contains(CellFlags::INVERSE),
        "wide": f.contains(CellFlags::WIDE),
    }))
}

fn not_exported(name: &str) -> String {
    format!("ui.rect {name}: the window does not export that rectangle yet")
}

fn ui_rect(ctx: &ProbeCtx, arg: &Value) -> Result<Value, String> {
    let name = arg
        .get("name")
        .and_then(Value::as_str)
        .ok_or("ui.rect needs a name")?;
    let a = ctx.app;
    let k = scale_of(ctx);
    let (w, h) = css_size(ctx);
    // The title buttons, from the right edge: close, maximise, minimise, settings, command help.
    let button = |from_right: usize| {
        let x = w - BTN_RIGHT - BTN_W - from_right as f32 * (BTN_W + BTN_GAP);
        rect_json(x, (BAR_H - BTN_H) / 2.0, BTN_W, BTN_H, k)
    };
    if let Some(n) = name.strip_prefix("tab:") {
        let n: usize = n
            .parse()
            .map_err(|_| format!("ui.rect {name}: not a tab number"))?;
        let rows = rows_of(ctx);
        let row = rows
            .get(n)
            .ok_or_else(|| format!("ui.rect {name}: no such tab"))?;
        return match tab_rect(ctx, row) {
            Value::Null => Err(not_exported(name)),
            v => Ok(v),
        };
    }
    match name {
        "close" => Ok(button(0)),
        "max" => Ok(button(1)),
        "min" => Ok(button(2)),
        "cog" => Ok(button(3)),
        "help" if a.get_help_visible() => Ok(button(4)),
        "help" => Err("ui.rect help: the command help button is not showing".into()),
        "mark" if !a.get_layout_vertical() => Ok(rect_json(0.0, 0.0, mark_w(k), BAR_H, k)),
        "strip" if !a.get_layout_vertical() => Ok(rect_json(
            mark_w(k),
            0.0,
            a.get_strip_phys_width() as f32 / k,
            BAR_H - hair(k),
            k,
        )),
        "grid" => Ok(rect_json(
            a.get_grid_left(),
            a.get_grid_top(),
            a.get_grid_phys_width() as f32 / k,
            a.get_grid_phys_height() as f32 / k,
            k,
        )),
        "sidebar" if a.get_layout_vertical() && !a.get_sidebar_collapsed() => {
            Ok(rect_json(0.0, BAR_H, a.get_sidebar_width(), h - BAR_H, k))
        }
        "sidebar" => Err("ui.rect sidebar: the sidebar is not open".into()),
        "tip" if a.get_tip_visible() => Ok(tip_rect(ctx)),
        "tip" => Err("ui.rect tip: no tooltip is showing".into()),
        "menu" if a.get_menu_open() => Ok(rect_json(
            a.get_menu_panel_x(),
            a.get_menu_panel_y(),
            a.get_menu_w(),
            a.get_menu_h(),
            k,
        )),
        "menu" => Err("ui.rect menu: no menu is open".into()),
        "find" if a.get_find_open() => Ok(rect_json(
            a.get_find_bar_x(),
            a.get_find_bar_y(),
            a.get_find_bar_w(),
            a.get_find_bar_h(),
            k,
        )),
        "find" => Err("ui.rect find: the find bar is not open".into()),
        "badge" if a.get_copied_phase() != 0 => Ok(rect_json(
            a.get_badge_x(),
            a.get_badge_y(),
            a.get_badge_w(),
            a.get_badge_h(),
            k,
        )),
        "badge" => Err("ui.rect badge: the Copied badge is not showing".into()),
        "dialog" if a.get_dialog_open() => Ok(rect_json(
            a.get_dialog_box_x(),
            a.get_dialog_box_y(),
            a.get_dialog_box_w(),
            a.get_dialog_box_h(),
            k,
        )),
        "dialog" => Err("ui.rect dialog: no dialog is open".into()),
        "start" if a.get_front_kind() == 2 => Ok(rect_json(
            a.get_start_x(),
            a.get_start_y(),
            a.get_start_w(),
            a.get_start_h(),
            k,
        )),
        "start" => Err("ui.rect start: the start page is not showing".into()),
        // The strip the terminal's scrollbar sits in: 6 css px wide on the grid's right edge.
        "scrollbar" if a.get_scroll_on() => Ok(rect_json(
            a.get_scroll_strip_x(),
            a.get_grid_top(),
            a.get_scroll_strip_w(),
            a.get_grid_phys_height() as f32 / k,
            k,
        )),
        "scrollbar" => Err("ui.rect scrollbar: the shell has no history to scroll".into()),
        "strip" | "mark" => Err(not_exported(name)),
        _ => Err(format!("ui.rect: no such rectangle: {name}")),
    }
}

fn clip_last(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let c = ctx.clip.ok_or("no clipboard stand-in in this run")?;
    Ok(json!({ "text": c.read_text(), "seq": c.sequence() }))
}

fn open_log(_ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    use crate::openers::OpenedRecord as R;
    let rows: Vec<Value> = crate::openers::recorded()
        .into_iter()
        .map(|r| match r {
            R::Open(p) => json!({ "kind": "open", "path": p }),
            R::Reveal(p) => json!({ "kind": "reveal", "path": p }),
            R::Folder(p) => json!({ "kind": "folder", "path": p }),
        })
        .collect();
    Ok(json!({ "calls": rows }))
}

/// How many times a bell asked the taskbar button to flash under `--e2e` (the real flash is never
/// made there: the window is parked and has no button).
static FLASHES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// A bell flashed the taskbar button of a window nobody is looking at.
pub fn record_flash() {
    FLASHES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// `flash.count`: how many flashes the bells asked for.
fn flash_count(_ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    Ok(json!({ "count": FLASHES.load(std::sync::atomic::Ordering::SeqCst) }))
}

fn startup_trace(_ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let phases: Vec<Value> = crate::startup_trace::snapshot()
        .iter()
        .map(|(p, d)| json!({ "name": p.name(), "ms": d.as_secs_f64() * 1000.0 }))
        .collect();
    let (prompt, usable) = crate::e2e_glue::prompt_marks();
    Ok(json!({
        "line": crate::startup_trace::line(),
        "phases": phases,
        "prompt_ms": prompt,
        "usable_ms": usable,
    }))
}

/// The terminal's padding in device pixels at `scale` (`TERM_PADDING_LOGICAL`, rounded).
fn padding_px(scale: f32) -> u32 {
    (flare_render::metrics::TERM_PADDING_LOGICAL * scale).round() as u32
}

fn dpi_state(ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let k = scale_of(ctx);
    let size = ctx.app.window().size();
    let (cols, rows) = shell_of(ctx, &Value::Null)
        .map(|(_, h)| h.size())
        .unwrap_or((0, 0));
    let pad = padding_px(k);
    let gw = ctx.app.get_grid_phys_width().max(0) as u32;
    let gh = ctx.app.get_grid_phys_height().max(0) as u32;
    // The cell is what the grid's pixels leave after the padding, shared by the cells: exact to
    // the pixels left over (less than one per cell).
    let noted = crate::e2e_hook::cell();
    let cell = |px: u32, n: usize| {
        if n == 0 {
            Value::Null
        } else {
            Value::from(px.saturating_sub(2 * pad) / n as u32)
        }
    };
    Ok(json!({
        "scale": k,
        "grid_cols": cols,
        "grid_rows": rows,
        "cell_w": noted.map_or_else(|| cell(gw, cols), |(w, _)| Value::from(w)),
        "cell_h": noted.map_or_else(|| cell(gh, rows), |(_, h)| Value::from(h)),
        "padding": pad,
        "client_w_px": size.width,
        "client_h_px": size.height,
        "hair_px": (k.floor() as u32).max(1),
    }))
}

fn probe_names(_ctx: &ProbeCtx, _arg: &Value) -> Result<Value, String> {
    let mut r = ProbeRegistry::new();
    super::register_all(&mut r);
    Ok(json!(r.names()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeSet;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use flare_settings::kv::PrefStore;
    use flare_tabs::restore::{NoSessions, SavedTab, SavedTabs, tabs_json};
    use flare_tabs::tabs_store::TabsStore;
    use flare_term::fake::{FakeShells, FakeSpawner};
    use flare_win::window_state::WindowStateStore;
    use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

    use crate::appctl::{AppCtl, Deps, Post, UiMsg};
    use crate::config::Args;
    use crate::tab_host::TabHost;
    use crate::ui_harness::{Harness, SCALES};
    use crate::{AppWindow, e2e_probes};

    type Queue = Arc<Mutex<VecDeque<UiMsg>>>;

    struct Fx {
        h: Harness<AppWindow>,
        ctl: Rc<RefCell<AppCtl>>,
        q: Queue,
        _dir: tempfile::TempDir,
        args: Args,
        reg: ProbeRegistry,
    }

    fn grid_image() -> Image {
        let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(8, 8);
        for p in buf.make_mut_slice() {
            *p = Rgba8Pixel {
                r: 20,
                g: 20,
                b: 20,
                a: 255,
            };
        }
        Image::from_rgba8(buf)
    }

    fn args() -> Args {
        crate::config::parse_args(&["--e2e".to_string()], None).unwrap()
    }

    impl Fx {
        fn new(scale: f32, tabs: usize) -> Fx {
            let dir = tempfile::tempdir().unwrap();
            let saved = SavedTabs {
                tabs: (1..=tabs)
                    .map(|i| SavedTab {
                        cwd: format!("C:\\w\\proj{i:02}"),
                        agent: None,
                    })
                    .collect(),
                active: 0,
            };
            std::fs::write(dir.path().join("tabs.json"), tabs_json(&saved)).unwrap(); // sync-ok: test code only, never the app
            let prefs = PrefStore::memory();
            let q: Queue = Arc::default();
            let post: Post = {
                let q = q.clone();
                Arc::new(move |m| q.lock().unwrap().push_back(m))
            };
            let waker: Arc<dyn Fn() + Send + Sync> = {
                let post = post.clone();
                Arc::new(move || post(UiMsg::HostWake))
            };
            let host = TabHost::new(
                FakeSpawner::new(),
                FakeShells::new(),
                prefs.clone(),
                waker,
                None,
            );
            let clock = Rc::new(Cell::new(0u64));
            let deps = Deps {
                prefs: prefs.clone(),
                tabs_store: Arc::new(TabsStore::new(
                    dir.path().join("tabs.json"),
                    Duration::from_millis(30),
                )),
                window_store: Arc::new(WindowStateStore::new(
                    dir.path().join("window.json"),
                    Duration::from_millis(30),
                )),
                host,
                lookup: Arc::new(NoSessions),
                is_dir: Arc::new(|_| true),
                home: "C:\\home".to_string(),
                clock: Rc::new(move || clock.get()),
                post,
                pick_folder: Rc::new(|_, done| done(None)),
                clipboard: Rc::new(|_| {}),
                open_url: Rc::new(|_| {}),
                set_grid_source: Rc::new(|_| {}),
                raise: Rc::new(|| {}),
                quit: Rc::new(|| {}),
                hide: Rc::new(|| {}),
                launch_folders: Vec::new(),
                e2e: true,
                term: crate::appctl::TermPorts::inert(),
            };
            let h = Harness::new(scale, 1000, 640, || AppWindow::new().unwrap());
            h.comp.set_grid_image(grid_image());
            let ctl = AppCtl::new(&h.comp, deps);
            {
                let weak = Rc::downgrade(&ctl);
                h.comp.on_grid_resized(move |w, hgt, _s| {
                    if let Some(rc) = weak.upgrade()
                        && let Ok(mut c) = rc.try_borrow_mut()
                    {
                        c.on_grid_dims((w / 8).max(2) as usize, (hgt / 16).max(1) as usize);
                    }
                });
            }
            h.advance(0);
            ctl.borrow_mut().refresh();
            let mut reg = ProbeRegistry::new();
            e2e_probes::register_all(&mut reg);
            let fx = Fx {
                h,
                ctl,
                q,
                _dir: dir,
                args: args(),
                reg,
            };
            AppCtl::start_restore(&fx.ctl);
            let t = std::time::Instant::now();
            while !fx.ctl.borrow().model().restore_done() {
                assert!(
                    t.elapsed() < Duration::from_secs(5),
                    "the restore timed out"
                );
                fx.pump();
                std::thread::sleep(Duration::from_millis(5));
            }
            fx.pump();
            fx.ctl.borrow_mut().refresh();
            fx.h.advance(0);
            fx
        }

        fn pump(&self) {
            loop {
                let m = self.q.lock().unwrap().pop_front();
                let Some(m) = m else { break };
                self.ctl.borrow_mut().on_msg(m);
            }
            self.h.advance(0);
        }

        fn probe(&self, name: &str, arg: Value) -> Result<Value, String> {
            let ctx = ProbeCtx {
                app: &self.h.comp,
                ctl: &self.ctl,
                args: &self.args,
                clip: None,
                front: {
                    let c = self.ctl.borrow();
                    c.model()
                        .state
                        .active_id
                        .as_ref()
                        .and_then(|id| c.handle_of(id))
                },
            };
            self.reg.call(name, &ctx, &arg)
        }
    }

    #[test]
    fn every_registered_probe_name_is_unique_and_dotted() {
        let mut r = ProbeRegistry::new();
        e2e_probes::register_all(&mut r);
        let names = r.names();
        assert!(!names.is_empty());
        let set: BTreeSet<_> = names.iter().collect();
        assert_eq!(set.len(), names.len(), "a name is registered twice");
        for n in &names {
            assert!(n.contains('.'), "{n} is not dotted");
            assert!(
                n.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_'),
                "{n} has odd characters"
            );
        }
        for want in [
            "app.state",
            "grid.text",
            "grid.cell",
            "ui.rect",
            "clip.last",
            "open.log",
            "flash.count",
            "startup.trace",
            "dpi.state",
            "probe.names",
        ] {
            assert!(names.contains(&want), "{want} is missing");
        }
    }

    #[test]
    fn an_unknown_probe_is_an_error() {
        let f = Fx::new(1.0, 1);
        assert_eq!(
            f.probe("no.such", Value::Null).unwrap_err(),
            "no such probe: no.such"
        );
    }

    #[test]
    fn app_state_lists_tabs_in_strip_order_with_the_front_one_marked() {
        let f = Fx::new(1.0, 3);
        let second = f.ctl.borrow().model().state.tabs[1].id.clone();
        f.ctl.borrow_mut().model_mut().pick(&second);
        f.ctl.borrow_mut().refresh();
        f.h.advance(0);
        let s = f.probe("app.state", Value::Null).unwrap();
        let tabs = s["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 3);
        let fronts: Vec<bool> = tabs.iter().map(|t| t["front"].as_bool().unwrap()).collect();
        assert_eq!(fronts, [false, true, false]);
        assert_eq!(s["front"], tabs[1]["id"]);
        let cwds: Vec<&str> = tabs.iter().map(|t| t["cwd"].as_str().unwrap()).collect();
        assert_eq!(cwds, ["C:\\w\\proj01", "C:\\w\\proj02", "C:\\w\\proj03"]);
        assert_eq!(tabs[0]["kind"], "shell");
        assert_eq!(tabs[0]["mark"]["state"], "none");
        assert!(tabs[0]["rect"]["w"].as_f64().unwrap() > 0.0);
        assert!(s["window"]["w"].as_u64().unwrap() > 0);
    }

    #[test]
    fn app_state_reports_the_dialog_the_menu_and_the_find_bar() {
        let f = Fx::new(1.0, 2);
        let s = f.probe("app.state", Value::Null).unwrap();
        assert_eq!(s["dialog"]["open"], false);
        assert_eq!(s["menu"]["open"], false);
        assert_eq!(s["find"]["open"], false);

        // The dialog: the close question for a tab the app has to ask about is not needed here,
        // the window's own properties are what the probe reads.
        f.h.comp.set_dialog_title("Close?".into());
        f.h.comp.set_dialog_primary("Close tab".into());
        f.h.comp.set_dialog_open(true);
        let s = f.probe("app.state", Value::Null).unwrap();
        assert_eq!(s["dialog"]["open"], true);
        assert_eq!(s["dialog"]["title"], "Close?");
        assert_eq!(s["dialog"]["buttons"], json!(["Cancel", "Close tab"]));

        // The menu: a right click on the first tab opens the tab menu.
        f.h.comp.set_dialog_open(false);
        let r = f.probe("ui.rect", json!({ "name": "tab:0" })).unwrap();
        f.h.right_click(
            r["x"].as_f64().unwrap() as f32 + 5.0,
            r["y"].as_f64().unwrap() as f32 + 5.0,
        );
        f.h.advance(0);
        let s = f.probe("app.state", Value::Null).unwrap();
        assert_eq!(s["menu"]["open"], true);
        assert!(!s["menu"]["rows"].as_array().unwrap().is_empty());
        let m = f.probe("ui.rect", json!({ "name": "menu" })).unwrap();
        assert!(m["w"].as_f64().unwrap() > 0.0 && m["h"].as_f64().unwrap() > 0.0);

        // The find bar.
        f.h.comp.set_find_query("needle".into());
        f.h.comp.set_find_readout("1/3".into());
        f.h.comp.set_find_open(true);
        let s = f.probe("app.state", Value::Null).unwrap();
        assert_eq!(s["find"]["open"], true);
        assert_eq!(s["find"]["query"], "needle");
        assert_eq!(s["find"]["readout"], "1/3");
        let b = f.probe("ui.rect", json!({ "name": "find" })).unwrap();
        assert!(b["w"].as_f64().unwrap() > 0.0 && b["h"].as_f64().unwrap() > 0.0);

        // The dialog box and the badge answer once they show, and are errors while they do not.
        assert!(f.probe("ui.rect", json!({ "name": "dialog" })).is_err());
        f.h.comp.set_dialog_open(true);
        let d = f.probe("ui.rect", json!({ "name": "dialog" })).unwrap();
        assert!(d["w"].as_f64().unwrap() > 0.0);
        f.h.comp.set_dialog_open(false);
        assert!(f.probe("ui.rect", json!({ "name": "badge" })).is_err());
        f.h.comp.set_copied_phase(1);
        let g = f.probe("ui.rect", json!({ "name": "badge" })).unwrap();
        assert!(g["w"].as_f64().unwrap() > 0.0 && g["h"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn grid_text_returns_the_visible_rows_and_the_cursor() {
        let f = Fx::new(1.0, 1);
        let h = {
            let c = f.ctl.borrow();
            let id = c.model().state.active_id.clone().unwrap();
            c.handle_of(&id).expect("the front tab has a shell")
        };
        h.with_emu(|e| {
            e.feed(b"hi\r\n$ ");
        });
        let g = f.probe("grid.text", Value::Null).unwrap();
        let rows = g["rows"].as_array().unwrap();
        assert_eq!(rows[0], "hi");
        assert_eq!(rows[1], "$");
        assert_eq!(g["cursor"]["row"], 1);
        assert_eq!(g["cursor"]["col"], 2);
        assert_eq!(g["rows_n"].as_u64().unwrap() as usize, rows.len());
        let c = f.probe("grid.cell", json!({ "row": 0, "col": 1 })).unwrap();
        assert_eq!(c["ch"], "i");
        assert_eq!(c["bold"], false);
        assert!(
            f.probe("grid.cell", json!({ "row": 9999, "col": 0 }))
                .is_err()
        );
    }

    #[test]
    fn ui_rect_gives_device_pixels_that_are_css_times_scale_rounded() {
        for scale in SCALES {
            let f = Fx::new(scale, 2);
            for name in [
                "close", "max", "min", "cog", "tab:0", "tab:1", "grid", "strip",
            ] {
                let r = f.probe("ui.rect", json!({ "name": name })).unwrap();
                for (css, dev) in [("x", "dx"), ("y", "dy"), ("w", "dw"), ("h", "dh")] {
                    let want = (r[css].as_f64().unwrap() as f32 * scale).round() as i64;
                    assert_eq!(r[dev].as_i64().unwrap(), want, "{name}.{dev} at {scale}");
                }
                assert!(
                    r["w"].as_f64().unwrap() > 0.0,
                    "{name} has a width at {scale}"
                );
            }
            // The buttons sit in the title bar, close at the right edge, in a row.
            let close = f.probe("ui.rect", json!({ "name": "close" })).unwrap();
            let max = f.probe("ui.rect", json!({ "name": "max" })).unwrap();
            assert!(max["x"].as_f64().unwrap() < close["x"].as_f64().unwrap());
            assert!(f.probe("ui.rect", json!({ "name": "nonsense" })).is_err());
        }
    }

    #[test]
    fn dpi_state_follows_the_scale() {
        for scale in SCALES {
            let f = Fx::new(scale, 1);
            let d = f.probe("dpi.state", Value::Null).unwrap();
            assert_eq!(d["scale"].as_f64().unwrap() as f32, scale);
            assert_eq!(
                d["hair_px"].as_u64().unwrap(),
                (scale.floor() as u64).max(1)
            );
            assert_eq!(
                d["padding"].as_u64().unwrap(),
                (4.0 * scale).round() as u64,
                "padding at {scale}"
            );
            assert_eq!(
                d["client_w_px"].as_u64().unwrap(),
                (1000.0 * scale).round() as u64
            );
            assert_eq!(
                d["client_h_px"].as_u64().unwrap(),
                (640.0 * scale).round() as u64
            );
            assert!(d["grid_cols"].as_u64().unwrap() > 0);
        }
    }

    #[test]
    fn startup_and_clip_and_open_probes_answer_without_a_window_side_effect() {
        let f = Fx::new(1.0, 1);
        let t = f.probe("startup.trace", Value::Null).unwrap();
        assert!(t["line"].is_string());
        assert!(
            f.probe("clip.last", Value::Null).is_err(),
            "no clipboard stand-in here"
        );
        assert_eq!(f.probe("flash.count", Value::Null).unwrap()["count"], 0);
        assert!(f.probe("open.log", Value::Null).unwrap()["calls"].is_array());
        let names = f.probe("probe.names", Value::Null).unwrap();
        assert!(names.as_array().unwrap().iter().any(|n| n == "app.state"));
    }

    #[test]
    fn probe_files_never_mutate_the_controller() {
        let banned = [
            ["borrow", "_mut"].concat(),
            [".set", "_"].concat(),
            ["invoke", "_"].concat(),
        ];
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/e2e_probes");
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            // sync-ok: test code only, never the app
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap(); // sync-ok: test code only, never the app
            // Only the code before the test module counts.
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            for b in &banned {
                assert!(
                    !code.contains(b.as_str()),
                    "{} uses {b:?}: a probe only looks",
                    path.display()
                );
            }
            seen += 1;
        }
        assert!(seen >= 20, "the scan saw only {seen} files");
    }
}
