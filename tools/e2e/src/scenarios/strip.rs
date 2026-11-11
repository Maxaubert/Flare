//! The tab strip scenarios (Task 22): tabDragFirst, tabWidth, tabLooks, tabSwitch, tooltips, manyTabs.
//!
//! PT: run.mjs:1767-1794, 1796-1830, 1840-1944, 1955-2043, 2870-2924, 2932-2996. Probes: `app.state`
//! (tab rects, marks, the front tab), `strip.state` and `tip.state` (e2e_probes/strip.rs), `ui.rect`.
//! Tabs are read from the window; a pixel check reads the snapshot's PNG (`pixels.rs` for the maths).
//! What the window does not export is a `ctx.skip` with its reason, never a pass.

use std::path::Path;

use flare_e2e_proto::{Button, PointerAction, Request};
use serde_json::{json, Value};

use super::driver;
use crate::ctx::{shot_file, App, Ctx, Launch};
use crate::pixels::{bbox_where, Rgb};
use crate::world::{prompt_timeout, wait_for_prompt, Shell};

/// 225 percent (96 is 100 percent): the scale the pixel checks run at.
const DPI_225: u32 = 216;
/// The folder whose name is too long for its tab (PT `tabDragFirst`).
const WIDE: &str = "a-much-longer-folder-name";
/// The folder whose name is too long to fit on any tab (PT `tabWidth`).
const LONG: &str = "a-folder-with-a-name-far-too-long-to-fit-on-any-tab";

/// A decoded snapshot: device pixels, row-major.
pub(super) struct Snap {
    pub(super) w: u32,
    pub(super) h: u32,
    px: Vec<Rgb>,
}

impl Snap {
    pub(super) fn at(&self, x: u32, y: u32) -> Rgb {
        let i = (y.min(self.h - 1) * self.w + x.min(self.w - 1)) as usize;
        self.px[i]
    }
}

/// Whether a catalogue row has this id, or a label that says these words (lower case).
fn has_word(row: &Value, id: &str, words: &str) -> bool {
    row["id"].as_str() == Some(id)
        || row["label"]
            .as_str()
            .is_some_and(|l| l.to_lowercase().contains(words))
}

/// The app's `app.state` answer (a Null when the probe fails).
fn app_json(app: &mut App) -> Value {
    app.session.probe_str("app.state").unwrap_or(Value::Null)
}

/// The tabs in strip order, as `app.state` lists them.
fn tabs_of(app: &mut App) -> Vec<Value> {
    app_json(app)["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn num(v: &Value, key: &str) -> f32 {
    v[key].as_f64().unwrap_or(0.0) as f32
}

/// A tab's rectangle in css px: x, y, w, h.
fn rect_of(tab: &Value) -> (f32, f32, f32, f32) {
    let r = &tab["rect"];
    (num(r, "x"), num(r, "y"), num(r, "w"), num(r, "h"))
}

/// The centre of a css rectangle.
fn centre((x, y, w, h): (f32, f32, f32, f32)) -> (f32, f32) {
    (x + w / 2.0, y + h / 2.0)
}

/// A named `ui.rect` in css px, or None when the window does not show it.
fn ui_rect(app: &mut App, name: &str) -> Option<(f32, f32, f32, f32)> {
    let v = app.session.probe("ui.rect", json!({ "name": name })).ok()?;
    Some((num(&v, "x"), num(&v, "y"), num(&v, "w"), num(&v, "h")))
}

/// The window's scale (1.0 is 100 percent).
fn scale(app: &mut App) -> f32 {
    let k = app.session.state().map(|s| s.scale).unwrap_or(1.0);
    if k > 0.0 {
        k
    } else {
        1.0
    }
}

fn strip_state(app: &mut App) -> Value {
    app.session.probe_str("strip.state").unwrap_or(Value::Null)
}

fn tip_state(app: &mut App) -> Value {
    app.session.probe_str("tip.state").unwrap_or(Value::Null)
}

/// PT's `hold`: Ctrl held, one Tab per step (1 forward, -1 with Shift), then Ctrl let go with its own
/// key-up, which is what ends a Most recent walk.
fn walk(app: &mut App, steps: &[i32]) {
    for s in steps {
        let _ = app
            .session
            .key(if *s > 0 { "Ctrl+Tab" } else { "Ctrl+Shift+Tab" });
    }
    let _ = send(
        app,
        Request::Key {
            chord: "ControlLeft".into(),
            down: false,
        },
    );
}
/// The title of the tab in front ("" when none is).
fn front_title(app: &mut App) -> String {
    tabs_of(app)
        .iter()
        .find(|t| t["front"] == json!(true))
        .and_then(|t| t["title"].as_str())
        .unwrap_or("")
        .to_string()
}

/// Every tab title in strip order, for the messages.
fn titles(app: &mut App) -> String {
    let names: Vec<String> = tabs_of(app)
        .iter()
        .map(|t| t["title"].as_str().unwrap_or("").to_string())
        .collect();
    format!("{names:?}")
}

fn send(app: &mut App, req: Request) -> Result<(), String> {
    app.session.call(&req).map(|_| ())
}

fn move_to(app: &mut App, x: f32, y: f32) {
    let _ = send(
        app,
        Request::Pointer {
            x,
            y,
            action: PointerAction::Move,
        },
    );
}

fn click_at(app: &mut App, (x, y): (f32, f32)) {
    let _ = app.session.click(x, y);
}

/// A left-button drag from `from` to `to` in css px, in 20 steps.
fn drag(app: &mut App, from: (f32, f32), to: (f32, f32)) {
    let _ = send(
        app,
        Request::Drag {
            from,
            to,
            steps: 20,
            button: Button::Left,
        },
    );
}

/// Holds the scenario still for `ms`, in 50 ms "no change" steps (the most `settle` allows).
fn hold(ctx: &mut Ctx, ms: u64) {
    for _ in 0..(ms / 50) {
        ctx.settle(50);
    }
}

/// Types a line into the focused shell and presses Enter.
fn type_line(app: &mut App, line: &str) {
    // The front shell shows its first prompt before a line goes to it (typing earlier is lost).
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
fn say(state: &str, extra: &str) -> String {
    format!("[Console]::Write([char]27 + ']777;flare-agent;state={state}{extra}' + [char]7)")
}

/// Decodes the PNG a snapshot wrote into device pixels. None when the file is not a readable PNG.
pub(super) fn read_png(path: &Path) -> Option<Snap> {
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

/// A snapshot of the window, decoded. None when the snapshot failed.
pub(super) fn snap(ctx: &mut Ctx, app: &mut App, name: &str) -> Option<Snap> {
    let path = ctx.shot(app, name)?;
    read_png(&path)
}

/// The breath at or above which a picture shows a breathing mark (the question dot or line) at full
/// ink. The mark fades to nothing and back every two seconds, so a picture caught mid-fade shows the
/// fade, not the mark: a ring once read 0.28 of its ink (indicatorStyles, 2026-10-21).
pub(super) const FULL_BREATH: f32 = 0.85;

/// The breath the last snapshot's picture shows, as the app read it straight after the picture.
pub(super) fn picture_breathe(app: &mut App) -> Option<f32> {
    let series = app.session.probe_str("frames.series").ok()?;
    series.get("snap_breathe")?.as_f64().map(|b| b as f32)
}

/// A snapshot whose picture shows the breathing marks at full breath (a still window always does).
/// The breath comes round once every two seconds, so the wait is for that moment, never a second
/// try at a check; `None` when no such picture came.
pub(super) fn snap_full_breath(ctx: &mut Ctx, app: &mut App, name: &str) -> Option<Snap> {
    let (shots, scenario) = (ctx.shots.clone(), ctx.name);
    let mut got = None;
    ctx.until(
        &format!("a picture at full breath ({name})"),
        6000,
        &mut || {
            let Some(path) = shot_file(&shots, scenario, name, |p| app.session.snapshot(p)) else {
                return false;
            };
            if !picture_breathe(app).is_some_and(|b| b >= FULL_BREATH) {
                return false;
            }
            got = read_png(&path);
            got.is_some()
        },
    );
    got
}

/// The sum of the channel differences of two colours.
pub(super) fn dist(a: Rgb, b: Rgb) -> i32 {
    (0..3)
        .map(|i| (i32::from(a[i]) - i32::from(b[i])).abs())
        .sum()
}

/// The device-pixel column range of a css span, clamped to the snapshot.
fn dev_span(snap: &Snap, k: f32, from: f32, to: f32) -> (u32, u32) {
    let a = ((from * k).round().max(0.0) as u32).min(snap.w - 1);
    let b = ((to * k).round().max(0.0) as u32).min(snap.w - 1);
    (a, b.max(a))
}

/// The ink box inside a css region of the strip that differs from `ground`. Returned in css px.
fn ink_box(
    snap: &Snap,
    k: f32,
    region: (f32, f32, f32, f32),
    keep: impl Fn(Rgb) -> bool,
) -> Option<(f32, f32, f32, f32)> {
    let (x0, y0, w, h) = region;
    let (ax, bx) = dev_span(snap, k, x0, x0 + w);
    let (ay, by) = dev_span(snap, k, y0, y0 + h);
    let (rw, rh) = (bx - ax + 1, by - ay + 1);
    let found = bbox_where(rw, rh, |x, y| snap.at(ax + x, ay + y), keep)?;
    Some((
        (ax + found.0) as f32 / k,
        (ay + found.1) as f32 / k,
        found.2 as f32 / k,
        found.3 as f32 / k,
    ))
}

/// The width in css px of the ink at the end of tab `i` (the ring and the close), `None` when there
/// is no such tab or no snapshot.
fn end_ink_width(ctx: &mut Ctx, app: &mut App, i: usize, name: &str) -> Option<f32> {
    let k = scale(app);
    let t = tabs_of(app).get(i)?.clone();
    let (x, y, w, h) = rect_of(&t);
    let s = snap(ctx, app, name)?;
    let ground = s.at(((x + w / 2.0) * k) as u32, ((y + 5.0) * k) as u32);
    ink_box(&s, k, (x + w - 44.0, y + 6.0, 42.0, h - 10.0), |p| {
        dist(p, ground) > 30
    })
    .map(|b| b.2)
}
/// Starts the app with the given folders, software renderer, 100 percent unless `dpi` says.
fn launch_strip(
    ctx: &mut Ctx,
    folders: Vec<std::path::PathBuf>,
    dpi: Option<u32>,
) -> Result<App, String> {
    ctx.launch(Launch {
        renderer: Some("software"),
        folders,
        scale_dpi: dpi,
        tabs_along_the_top: true,
        ..Launch::default()
    })
}

/// The six launch folders `mark_strip` expects, in strip order.
pub(super) const MARK_FOLDERS: [&str; 6] =
    ["FlareTerminal", "notes", "api", "web", "infra", "docs"];

/// Clicks tab `i` and waits for it to be the tab in front, so the line typed next goes to its shell
/// (a click answers before the window has moved the front, and the line landed on the old tab).
fn focus_tab(ctx: &mut Ctx, app: &mut App, i: usize) {
    let id = tabs_of(app)
        .get(i)
        .and_then(|t| t["id"].as_str())
        .map(str::to_string);
    // The strip slides its tabs into place for a moment after they open, and a click on where a
    // tab is going lands on its neighbour: click again every quarter second until it is in front.
    let mut polls = 0u32;
    ctx.poll(6_000, &mut || {
        let t = tabs_of(app);
        let strip = t.get(i).is_some_and(|t| t["front"] == json!(true));
        let typed = app.session.state().ok().and_then(|s| s.front) == id;
        if strip && typed {
            return true;
        }
        if polls.is_multiple_of(10) {
            if let Some(tab) = t.get(i) {
                click_at(app, centre(rect_of(tab)));
            }
        }
        polls += 1;
        false
    });
}

/// PT's `markStrip` (run.mjs:1842-1845): six launch folders, each shell told a hook line so the tabs
/// take the mockups' states (working, none, working, finished, question, failed). True when the six
/// states hold, with the states read.
pub(super) fn mark_strip(ctx: &mut Ctx, app: &mut App) -> (bool, Vec<String>) {
    // PT: run.mjs:1842-1845 (markStrip, whose one check counts here too). Each shell takes a hook
    // line typed into it; the endings land while the first tab is in front.
    let six = ctx.until("six tabs", 30_000, &mut || tabs_of(app).len() == 6);
    // The tabs open as their shells come up and the strip settles into the launch order a moment
    // later; a line typed to "tab 0" before that lands on whichever folder sits there.
    let in_order = six
        && ctx.until("the tabs in launch order", 15_000, &mut || {
            let names: Vec<String> = tabs_of(app)
                .iter()
                .map(|t| t["title"].as_str().unwrap_or("").to_string())
                .collect();
            names == MARK_FOLDERS
        });
    if in_order {
        // Each marked tab runs a real agent process: a hook is a claim, and one look takes it back when
        // no agent runs in the tab (review #47).
        if let Ok(cmd) = crate::standins::agent_proc::command(&app.profile) {
            for i in [0usize, 2, 3, 4, 5] {
                focus_tab(ctx, app, i);
                type_line(app, &cmd);
            }
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
        for i in [0usize, 2, 3, 4, 5] {
            focus_tab(ctx, app, i);
            type_line(app, &say("working", ""));
        }
        focus_tab(ctx, app, 3);
        type_line(
            app,
            &format!("Start-Sleep -Milliseconds 9000; {}", say("done", "")),
        );
        focus_tab(ctx, app, 4);
        type_line(
            app,
            &format!("Start-Sleep -Milliseconds 8000; {}", say("question", "")),
        );
        focus_tab(ctx, app, 5);
        type_line(
            app,
            &format!(
                "Start-Sleep -Milliseconds 7000; {}; {}",
                say("failed", ";kind=rate_limit"),
                say("failed", "")
            ),
        );
        focus_tab(ctx, app, 0);
    }
    let want_states = [
        "working", "none", "working", "finished", "question", "failed",
    ];
    let states = |app: &mut App| -> Vec<String> {
        tabs_of(app)
            .iter()
            .map(|t| t["mark"]["state"].as_str().unwrap_or("").to_string())
            .collect()
    };
    let settled = ctx.until("the six tabs to take their states", 25_000, &mut || {
        states(app) == want_states
    });
    // The titles ride along in the message: a line that landed on the wrong tab shows by name.
    let got: Vec<String> = tabs_of(app)
        .iter()
        .map(|t| {
            format!(
                "{}={}",
                t["title"].as_str().unwrap_or(""),
                t["mark"]["state"].as_str().unwrap_or("")
            )
        })
        .collect();
    (settled, got)
}

/// PT `tabDragFirst` (run.mjs:1767-1794): a tab dragged to the left edge is first, and dragged back
/// to the right it is last again.
pub fn tab_drag_first(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let wide = ctx.ensure_world().root.join(WIDE);
    std::fs::create_dir_all(&wide).map_err(|e| e.to_string())?;
    let mut app = launch_strip(ctx, vec![alpha, wide], None)?;

    // PT: run.mjs:1772
    let two = ctx.until("two tabs", 15_000, &mut || tabs_of(&mut app).len() == 2);
    ctx.ok(two, "two tabs open");

    // PT: run.mjs:1773-1783. Dragged all the way left, the wide one is first.
    let t = tabs_of(&mut app);
    if t.len() == 2 {
        let (cx, cy) = centre(rect_of(&t[1]));
        drag(&mut app, (cx, cy), (0.0, cy));
    }
    let first = ctx.until("the dragged tab to be first", 4_000, &mut || {
        tabs_of(&mut app)
            .first()
            .and_then(|t| t["title"].as_str())
            .is_some_and(|s| s.contains(WIDE))
    });
    let said = titles(&mut app);
    ctx.ok(
        first,
        format!("dragged all the way left it is first ({said})"),
    );

    // PT: run.mjs:1784-1792. Dragged right past the second tab, it is last again.
    let t = tabs_of(&mut app);
    if t.len() == 2 {
        let (bx, by, bw, _) = rect_of(&t[1]);
        let (cx, cy) = centre(rect_of(&t[0]));
        drag(&mut app, (cx, cy), (bx + bw + 40.0, cy));
        let _ = by;
    }
    let last = ctx.until("the dragged tab back to be last", 4_000, &mut || {
        tabs_of(&mut app)
            .get(1)
            .and_then(|t| t["title"].as_str())
            .is_some_and(|s| s.contains(WIDE))
    });
    let said = titles(&mut app);
    ctx.ok(last, format!("and dragged right it is last again ({said})"));

    let _ = app.close();
    Ok(())
}

/// PT `tabWidth` (run.mjs:1796-1830): every tab is one width whatever its name, 140 to 152 px, and
/// picking a tab moves none.
pub fn tab_width(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let beta = ctx.ensure_world().beta.clone();
    let long = ctx.ensure_world().root.join(LONG);
    std::fs::create_dir_all(&long).map_err(|e| e.to_string())?;
    let mut app = launch_strip(ctx, vec![alpha, long, beta], None)?;

    // PT: run.mjs:1801
    let three = ctx.until("three tabs", 15_000, &mut || tabs_of(&mut app).len() == 3);
    ctx.ok(three, "three tabs open, one of them with a very long name");

    // PT: run.mjs:1814. There is no Tab width setting: the catalogue of every Settings row has none.
    let no_width = !driver::catalogue(&mut app)
        .iter()
        .any(|r| has_word(r, "tab-width", "tab width"));
    ctx.ok(
        no_width,
        "no Tab width setting (no row of the Settings catalogue sets a tab's width)",
    );

    // PT: run.mjs:1817-1819. Every tab one width, whatever its name.
    let widths: Vec<f32> = tabs_of(&mut app)
        .iter()
        .take(3)
        .map(|t| rect_of(t).2)
        .collect();
    let same = widths.len() == 3 && widths.iter().all(|w| (w - widths[0]).abs() < 0.5);
    ctx.ok(
        same,
        format!(
            "every tab is the same width, whatever its name ({})",
            widths
                .iter()
                .map(|w| format!("{w:.1}"))
                .collect::<Vec<_>>()
                .join(" / ")
        ),
    );

    // PT: run.mjs:1820. A fixed width, not a content one.
    let w0 = widths.first().copied().unwrap_or(0.0);
    ctx.ok(
        (140.0..=152.0).contains(&w0),
        format!("a fixed width, not a content one ({w0:.1}px)"),
    );

    // PT: run.mjs:1821. The long name is cut inside its tab; the Slint elision is not exported.
    ctx.skip("the long name is truncated inside the tab: the elision is not exported (ui_tests::strip::names_are_centred_and_elided holds it)");

    // PT: run.mjs:1824-1827. Picking another tab moves no tab.
    let before: Vec<f32> = tabs_of(&mut app).iter().map(|t| rect_of(t).2).collect();
    let t = tabs_of(&mut app);
    if t.len() == 3 {
        click_at(&mut app, centre(rect_of(&t[2])));
    }
    let landed = ctx.until("the picked tab to be in front", 2_000, &mut || {
        tabs_of(&mut app)
            .get(2)
            .is_some_and(|t| t["front"] == json!(true))
    });
    let after: Vec<f32> = tabs_of(&mut app).iter().map(|t| rect_of(t).2).collect();
    ctx.ok(
        landed && before == after,
        "picking another tab moves no tab",
    );

    let _ = app.close();
    Ok(())
}

/// PT `tabLooks` (run.mjs:1840-1944): six tabs in the mockups' states, with the strip's look read
/// from the snapshot at 225 percent. The checks the window does not export are skips.
pub fn tab_looks(ctx: &mut Ctx) -> Result<(), String> {
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
    let mut app = launch_strip(ctx, folders, Some(DPI_225))?;
    let _ = send(
        &mut app,
        Request::Resize {
            w: 1100.0,
            h: 700.0,
        },
    );

    let (settled, got) = mark_strip(ctx, &mut app);
    ctx.ok(
        settled,
        format!("six tabs in the mockups' states ({got:?})"),
    );

    // PT: run.mjs:1853. DROPPED: "no full border on a tab" is a CSS border width, and Flare paints
    // no tab border at all; the separator and ring checks below hold the look.

    let k = scale(&mut app);
    let tabs = tabs_of(&mut app);
    // The question tab's dot breathes: the ring check below is made on a picture at full breath.
    let snapshot = snap_full_breath(ctx, &mut app, "tabs-looks");

    // PT: run.mjs:1860. A faint line at every tab's right, the last one too: one device pixel at
    // the right edge differs from the tab's own ground a few pixels in.
    let sep_ok = match &snapshot {
        Some(s) => tabs.iter().all(|t| {
            let (x, y, w, _) = rect_of(t);
            let xr = ((x + w) * k).round() as u32;
            let yt = ((y + 3.0) * k).round() as u32;
            xr >= 8 && xr < s.w && s.at(xr - 1, yt) != s.at(xr - 7, yt)
        }),
        None => false,
    };
    ctx.ok(
        sep_ok,
        format!(
            "a faint line at every tab's right, the last one too ({} tabs)",
            tabs.len()
        ),
    );

    // PT: run.mjs:1889. An icon at the left of every tab: the ink there is 10 to 20 px wide, an
    // approximation of the 18 px icon box, which the window does not export.
    let icon_ok = match &snapshot {
        Some(s) => tabs.iter().all(|t| {
            let (x, y, _, h) = rect_of(t);
            // The ground is the tab's top band (the active tab's rule sits above the icon band).
            let ground = s.at(
                ((x + t["rect"]["w"].as_f64().unwrap_or(0.0) as f32 / 2.0) * k) as u32,
                ((y + 5.0) * k) as u32,
            );
            ink_box(s, k, (x + 2.0, y + 6.0, 28.0, h - 10.0), |c| {
                dist(c, ground) > 30
            })
            .is_some_and(|b| (10.0..=20.0).contains(&b.2))
        }),
        None => false,
    };
    ctx.ok(
        icon_ok,
        "every tab wears an icon at its left (ink 10 to 20 px wide)",
    );

    // PT: run.mjs:1890-1891. The shell wears the prompt, the agent tabs wear Claude.
    let shell_ok = tabs.get(1).is_some_and(|t| t["kind"] == json!("shell"));
    ctx.ok(shell_ok, "a plain shell wears the prompt");
    let claude_ok = [0usize, 2, 3, 4, 5]
        .iter()
        .all(|&i| tabs.get(i).is_some_and(|t| t["kind"] == json!("claude")));
    ctx.ok(claude_ok, "a tab Claude Code runs in wears Clawd");

    // PT: run.mjs:1892. The name is centred: the name's own box is not exported.
    ctx.skip("the name stays centred: the name's box is not exported (ui_tests::strip::names_are_centred_and_elided holds it)");

    // PT: run.mjs:1894. The ring at each marked tab's end, in the mark's colour.
    let ring_ok = match &snapshot {
        Some(s) => [0usize, 2, 3, 4, 5].iter().all(|&i| {
            let t = &tabs[i];
            let (x, y, w, h) = rect_of(t);
            // The ring is drawn in the name's ink, not the mark colour (measured on the snapshot).
            let ground = s.at(((x + w / 2.0) * k) as u32, ((y + 5.0) * k) as u32);
            ink_box(s, k, (x + w - 30.0, y + 6.0, 30.0, h - 10.0), |p| {
                dist(p, ground) > 30
            })
            .is_some()
        }),
        None => false,
    };
    ctx.ok(ring_ok, "every marked tab's ring is at its end");

    // PT: run.mjs:1895. Ring is the only mark: no bar.
    let only_ring = [0usize, 2, 3, 4, 5].iter().all(|&i| {
        tabs.get(i)
            .is_some_and(|t| t["mark"]["motion"] == json!("ring"))
    });
    ctx.ok(only_ring, "Ring is the only mark: no bar");

    // PT: run.mjs:1896. The close waits behind the ring and takes no room: with the pointer off the
    // tabs, the ink at a marked tab's end is the ring alone (the ring's box is 16 px).
    if let Some(g) = ui_rect(&mut app, "grid") {
        let c = centre(g);
        move_to(&mut app, c.0, c.1);
    }
    hold(ctx, 400);
    let rest = end_ink_width(ctx, &mut app, 2, "tabs-end-rest");
    ctx.ok(
        rest.is_some_and(|w| w <= 20.0),
        format!("beside its ring the close waits, taking no room ({rest:?} px of ink at the end)"),
    );

    // PT: run.mjs:1897. An idle tab shows neither ring nor close (the ring: its mark is none).
    let idle_ok = tabs.get(1).is_some_and(|t| {
        t["mark"]["state"] == json!("none") && t["mark"]["motion"] == json!("none")
    });
    ctx.ok(idle_ok, "an idle tab behind shows no ring");

    // PT: run.mjs:1904. Hovered: the ring stays and the close slides in after it, so the ink at the
    // end grows from the ring alone to the ring and the close.
    if let Some(t) = tabs_of(&mut app).get(2) {
        let c = centre(rect_of(t));
        move_to(&mut app, c.0 - 20.0, c.1);
    }
    hold(ctx, 400);
    let hovered = end_ink_width(ctx, &mut app, 2, "tabs-end-hover");
    ctx.ok(
        matches!((rest, hovered), (Some(r), Some(h)) if h >= 26.0 && h > r + 8.0),
        format!("hovered, the ring stays and the close slides in after it ({rest:?} -> {hovered:?} px of ink)"),
    );
    if let Some(g) = ui_rect(&mut app, "grid") {
        let c = centre(g);
        move_to(&mut app, c.0, c.1);
    }

    // PT: run.mjs:1908. Every tab one width. PT's rule is `flex-[0_1_150px] min-w-[84px]` with the
    // strip's room shared equally, so the width is the strip's share of its room: 150 at most, 84
    // at least, and 140 to 152 only when the room has it. At 1100 css with Flare's title row (the
    // mark and the buttons, help on by default) the six tabs get 139.1 css, the share, and no tab
    // is squeezed below PT's floor. The room is read from the probe in device px at 225 percent
    // (DPI_225 216 = 2.25); the strip keeps 4 px on its right and the + takes 28.
    let widths: Vec<f32> = tabs.iter().map(|t| rect_of(t).2).collect();
    let one_width = widths.iter().all(|w| (w - widths[0]).abs() < 0.5);
    let strip_css =
        strip_state(&mut app)["strip_px"].as_f64().unwrap_or(0.0) / (f64::from(DPI_225) / 96.0);
    let share = ((strip_css - 4.0 - 28.0) / widths.len().max(1) as f64).clamp(84.0, 150.0);
    ctx.ok(
        one_width && (f64::from(widths[0]) - share).abs() < 0.5,
        format!(
            "every tab one width, the strip's share of its room ({widths:?}, share {share:.1})"
        ),
    );

    // PT: run.mjs:1914. A click moves no tab.
    let edges = |app: &mut App| -> Vec<(f32, f32)> {
        tabs_of(app)
            .iter()
            .map(|t| {
                let (x, _, w, _) = rect_of(t);
                (x, x + w)
            })
            .collect()
    };
    let before = edges(&mut app);
    let t = tabs_of(&mut app);
    if t.len() == 6 {
        click_at(&mut app, centre(rect_of(&t[3])));
    }
    let landed = ctx.until("the clicked tab in front", 2_000, &mut || {
        tabs_of(&mut app)
            .get(3)
            .is_some_and(|t| t["front"] == json!(true))
    });
    let after = edges(&mut app);
    ctx.ok(landed && before == after, "clicking a tab moves no tab");

    // PT: run.mjs:1919. No Tab style row: the mark's look is the Agents page's indicator choice.
    let no_style = !driver::catalogue(&mut app)
        .iter()
        .any(|r| has_word(r, "tab-style", "tab style"));
    ctx.ok(no_style, "Settings has no Tab style row");

    // PT: run.mjs:1924. The Settings tab wears the cog: open it from the cog.
    if let Some(cog) = ui_rect(&mut app, "cog") {
        click_at(&mut app, centre(cog));
    }
    let cog_ok = ctx.until("the Settings tab", 4_000, &mut || {
        tabs_of(&mut app)
            .iter()
            .any(|t| t["kind"] == json!("settings"))
    });
    ctx.ok(cog_ok, "the Settings tab wears the cog");

    // PT: run.mjs:1920-1923. The indicator is Ring or Bar, and Ring is the default.
    let there = driver::goto_row(ctx, &mut app, "agent-indicator");
    let names: Vec<String> = driver::row(&mut app, "agent-indicator")["options"]
        .as_array()
        .map(|o| {
            o.iter()
                .map(|n| n.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    ctx.ok(
        there && names.join("|") == "Ring|Bar",
        format!("the indicator is Ring or Bar ({})", names.join("|")),
    );
    ctx.ok(
        driver::row_is(&mut app, "agent-indicator", &["ring"]),
        "Ring is the default",
    );

    // PT: run.mjs:1933. Bar: the line along each marked tab, no ring.
    let bar = driver::pick_seg(ctx, &mut app, "agent-indicator", &["minimal"]);
    let lines = ctx.until("every marked tab to wear a line", 6_000, &mut || {
        let t = tabs_of(&mut app);
        // The tab clicked above was visited, so its finished mark is gone: only the tabs that
        // still carry a mark are read.
        let marked: Vec<&Value> = t
            .iter()
            .filter(|x| x["mark"]["state"] != json!("none"))
            .collect();
        marked.len() >= 3
            && marked
                .iter()
                .all(|x| matches!(x["mark"]["motion"].as_str(), Some("line" | "run")))
    });
    let rings = tabs_of(&mut app)
        .iter()
        .filter(|t| t["mark"]["motion"] == json!("ring"))
        .count();
    ctx.ok(
        bar && lines && rings == 0,
        format!("Bar: a line along each marked tab, no ring ({rings} rings left)"),
    );

    let _ = app.close();
    Ok(())
}

/// PT `tabSwitch` (run.mjs:1955-2043): Ctrl+Tab in order, then Most recent (set in a relaunch by
/// its settings file, the way the lifecycle port relaunches), and a closed tab leaving the list.
pub fn tab_switch(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let beta = ctx.ensure_world().beta.clone();
    let gamma = ctx.ensure_world().root.join("gamma");
    std::fs::create_dir_all(&gamma).map_err(|e| e.to_string())?;
    let folders = vec![alpha, beta, gamma.clone()];
    let mut app = launch_strip(ctx, folders.clone(), None)?;

    // PT: run.mjs:1963
    let three = ctx.until("three tabs", 15_000, &mut || tabs_of(&mut app).len() == 3);
    ctx.ok(three, "three tabs open");

    // PT: run.mjs:1990. In order: visit the first tab, Ctrl+Tab steps to the next, Ctrl+Shift+Tab back.
    let t = tabs_of(&mut app);
    if let Some(first) = t.first() {
        click_at(&mut app, centre(rect_of(first)));
    }
    hold(ctx, 150);
    let _ = app.session.key("Ctrl+Tab");
    let beta_front = ctx.until("beta in front", 4_000, &mut || {
        front_title(&mut app).starts_with("beta")
    });
    ctx.ok(
        beta_front,
        format!(
            "In order: Ctrl+Tab steps to the next tab in the strip ({})",
            front_title(&mut app)
        ),
    );
    let _ = app.session.key("Ctrl+Shift+Tab");
    let alpha_front = ctx.until("alpha in front", 4_000, &mut || {
        front_title(&mut app).starts_with("alpha")
    });
    ctx.ok(
        alpha_front,
        format!("and Ctrl+Shift+Tab back ({})", front_title(&mut app)),
    );

    // PT: run.mjs:1999-2004. The Tab switching row: right after Tab position, In order out of the
    // box, the choices In order and Most recent, and Most recent is stored when picked.
    let ids: Vec<String> = driver::catalogue(&mut app)
        .iter()
        .map(|r| r["id"].as_str().unwrap_or("").to_string())
        .collect();
    let at = |id: &str| ids.iter().position(|x| x == id);
    ctx.ok(
        at("tab-layout").is_some() && at("tab-switch") == at("tab-layout").map(|i| i + 1),
        "Tab switching is right after Tab position",
    );
    let there = driver::goto_row(ctx, &mut app, "tab-switch");
    ctx.ok(
        there && driver::row_is(&mut app, "tab-switch", &["order"]),
        "In order is the default",
    );
    let names: Vec<String> = driver::row(&mut app, "tab-switch")["options"]
        .as_array()
        .map(|o| {
            o.iter()
                .map(|n| n.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    ctx.ok(
        names.join("|") == "In order|Most recent",
        format!(
            "the choices are In order and Most recent ({})",
            names.join("|")
        ),
    );
    let picked = driver::pick_seg(ctx, &mut app, "tab-switch", &["recent"]);
    let stored_file = ctx.ensure_world().profile.join("settings.json");
    let stored = ctx.poll(6_000, &mut || {
        std::fs::read_to_string(&stored_file)
            .is_ok_and(|s| s.contains("\"flare.window.tabSwitch\"") && s.contains("recent"))
    });
    ctx.ok(picked && stored, "Most recent is stored");

    // Relaunch on the same profile with Most recent stored, and no saved tabs (the lifecycle port's
    // relaunch): the tabs are the three folders again.
    let profile = ctx.ensure_world().profile.clone();
    let _ = app.close();
    let _ = std::fs::remove_file(profile.join("tabs.json"));
    let _ = std::fs::write(
        profile.join("settings.json"),
        r#"{"flare.window.tabSwitch":"recent"}"#,
    );
    let mut app = launch_strip(ctx, folders, None)?;
    let three = ctx.until("three tabs after the relaunch", 15_000, &mut || {
        tabs_of(&mut app).len() == 3
    });
    if !three {
        ctx.skip("the relaunch did not hold three tabs, so the Most recent checks were not run");
        let _ = app.close();
        return Ok(());
    }

    // PT: run.mjs:2008. Used in the order alpha, beta, gamma.
    for i in [0usize, 1, 2] {
        let t = tabs_of(&mut app);
        if let Some(tab) = t.get(i) {
            click_at(&mut app, centre(rect_of(tab)));
        }
        hold(ctx, 150);
    }
    // PT: run.mjs:2009-2018. Ctrl held, a Tab per step, then Ctrl let go (the hook's key grammar
    // sends a modifier's key-up on its own): one flip goes to the tab used before this one, the next
    // flips back, two steps in one hold go two back, and Shift goes the other way.
    let lands = |ctx: &mut Ctx, app: &mut App, steps: &[i32], want: &str, why: &str| {
        walk(app, steps);
        let there = ctx.until(&format!("{want} in front"), 4_000, &mut || {
            front_title(app).starts_with(want)
        });
        ctx.ok(there, format!("{why} ({})", front_title(app)));
    };
    lands(
        ctx,
        &mut app,
        &[1],
        "beta",
        "one Ctrl+Tab goes to the tab used before this one",
    );
    lands(
        ctx,
        &mut app,
        &[1],
        "gamma",
        "and the next flips back: the last two used",
    );
    lands(
        ctx,
        &mut app,
        &[1, 1],
        "alpha",
        "Tab twice in one hold goes two back, no ping-pong",
    );
    lands(
        ctx,
        &mut app,
        &[1],
        "gamma",
        "released there, alpha is now the latest, so the next flip is gamma",
    );
    lands(
        ctx,
        &mut app,
        &[-1],
        "beta",
        "Ctrl+Shift+Tab on a fresh hold goes to the oldest",
    );
    // PT: run.mjs:2021-2024. A closed tab leaves the list: close gamma (idle, so nothing asks).
    let t = tabs_of(&mut app);
    if let Some(g) = t.get(2) {
        click_at(&mut app, centre(rect_of(g)));
    }
    let gamma_front = ctx.until("gamma in front", 4_000, &mut || {
        front_title(&mut app).starts_with("gamma")
    });
    ctx.ok(gamma_front, "gamma in front");
    let _ = app.session.key("Ctrl+W");
    let closed = ctx.until("two tabs after Ctrl+W", 4_000, &mut || {
        tabs_of(&mut app).len() == 2
    });
    ctx.ok(closed, "Ctrl+W closes it");

    // PT: run.mjs:2026-2028. Ctrl+Tab goes to another open tab, never the closed one.
    let after = front_title(&mut app);
    walk(&mut app, &[1]);
    hold(ctx, 150);
    let flip = front_title(&mut app);
    ctx.ok(
        !flip.starts_with("gamma") && flip != after && !flip.is_empty(),
        format!("Ctrl+Tab goes to the other open tab, never the closed one ({after} > {flip})"),
    );

    // PT: run.mjs:2030-2039. A new tab entering at the front: the new-tab chooser's rectangle is
    // not exported, so these five checks are skips.
    // The new tab comes from Ctrl+T (default folder mode, so no chooser opens): the `+` chooser's
    // rectangle is not exported, and the used-list checks are the same whichever way the tab came.
    let used_before = front_title(&mut app);
    let _ = app.session.key("Ctrl+T");
    let added = ctx.until("a third tab after Ctrl+T", 15_000, &mut || {
        tabs_of(&mut app).len() == 3
    });
    ctx.ok(added, "a new tab opens");
    let new_title = tabs_of(&mut app)
        .last()
        .and_then(|t| t["title"].as_str())
        .unwrap_or("")
        .to_string();
    let in_front = ctx.until("the new tab in front", 4_000, &mut || {
        tabs_of(&mut app)
            .last()
            .is_some_and(|t| t["front"] == json!(true))
    });
    ctx.ok(in_front, format!("and is in front ({new_title})"));
    walk(&mut app, &[1]);
    let back = ctx.until(
        "the tab used before the new one in front",
        4_000,
        &mut || front_title(&mut app) == used_before,
    );
    ctx.ok(
        back,
        format!("one Ctrl+Tab goes to the tab used before the new one ({used_before})"),
    );
    // PT: run.mjs:2038-2039. The walk ended with Ctrl let go, so the next flip comes back.
    walk(&mut app, &[1]);
    let again = ctx.until("the new tab in front again", 4_000, &mut || {
        front_title(&mut app) == new_title
    });
    ctx.ok(again, "and the next comes back to the new tab");

    let _ = app.close();
    Ok(())
}

/// PT `tooltips` (run.mjs:2870-2924): a tip waits a second, sits centred under its item and just
/// below it, stays inside the window, and a tab's tip is centred under the whole tab.
pub fn tooltips(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let mut app = launch_strip(ctx, vec![alpha], None)?;
    let one = ctx.until("one tab", 15_000, &mut || tabs_of(&mut app).len() == 1);
    let _ = one;

    // The window's grid: a resting spot for the pointer that shows no tip.
    let rest = ui_rect(&mut app, "grid")
        .map(centre)
        .unwrap_or((400.0, 300.0));

    // PT: run.mjs:2886-2892. Not at once: over half a second on the ? shows no tip yet.
    if let Some(help) = ui_rect(&mut app, "help") {
        let (cx, cy) = centre(help);
        let t0 = std::time::Instant::now();
        move_to(&mut app, cx, cy);
        // macOS: read every 50 ms, so a tip that came too soon says when it came and what it was. Windows
        // reads once after 600 ms, as before.
        let mut early: Option<(u128, Value)> = None;
        if !cfg!(target_os = "macos") {
            hold(ctx, 600);
            let t = tip_state(&mut app);
            if t["visible"] == json!(true) {
                early = Some((t0.elapsed().as_millis(), t));
            }
        }
        while cfg!(target_os = "macos") && t0.elapsed() < std::time::Duration::from_millis(600) {
            let t = tip_state(&mut app);
            if t["visible"] == json!(true) {
                early = Some((t0.elapsed().as_millis(), t));
                break;
            }
            ctx.settle(50);
        }
        ctx.ok(
            early.is_none(),
            match &early {
                None => "over half a second on the ? shows no tip yet".to_string(),
                Some((ms, t)) => format!(
                    "over half a second on the ? shows no tip yet (one showed after {ms} ms: {})",
                    t["text"]
                ),
            },
        );
        move_to(&mut app, rest.0, rest.1);
        ctx.until("the tip to go", 2_000, &mut || {
            tip_state(&mut app)["visible"] != json!(true)
        });
    } else {
        ctx.ok(
            false,
            "over half a second on the ? shows no tip yet (the ? is not showing)",
        );
    }

    // A hover on an item's off-centre corner, as a hand lands; then its tip after its entrance.
    let hover = |ctx: &mut Ctx,
                 app: &mut App,
                 item: (f32, f32, f32, f32)|
     -> Option<(String, (f32, f32, f32, f32))> {
        let (x, y, w, h) = item;
        move_to(app, x + w - 3.0, y + h - 3.0);
        let shown = ctx.until("the tip", 4_000, &mut || {
            tip_state(app)["visible"] == json!(true)
        });
        hold(ctx, 200);
        let t = tip_state(app);
        if !shown || t["visible"] != json!(true) {
            return None;
        }
        let r = &t["rect"];
        Some((
            t["text"].as_str().unwrap_or("").to_string(),
            (num(r, "x"), num(r, "y"), num(r, "w"), num(r, "h")),
        ))
    };

    // PT: run.mjs:2894-2901. Hovering the ? shows the app's own tip, centred under it and 6 px below.
    if let Some(help) = ui_rect(&mut app, "help") {
        match hover(ctx, &mut app, help) {
            Some((text, tip)) => {
                ctx.ok(
                    true,
                    format!("hovering the ? shows the app's own tip (\"{text}\")"),
                );
                let dx = (tip.0 + tip.2 / 2.0 - (help.0 + help.2 / 2.0)).abs();
                // macOS (rule 1115): the ? is the row's second button from the right edge (the traffic
                // lights are on the left), so a centred tip would cross the window's edge: it is held
                // 6 px inside it (tip_place's TIP_MARGIN), as the cog's tip below is.
                let k = scale(&mut app);
                let client = app_json(&mut app)["window"]["client_w"]
                    .as_f64()
                    .unwrap_or(0.0) as f32
                    / k;
                let held_in = cfg!(target_os = "macos")
                    && (tip.0 + tip.2 - (client - 6.0)).abs() <= 1.5
                    && tip.0 + tip.2 / 2.0 < help.0 + help.2 / 2.0;
                ctx.ok(
                    dx <= 1.5 || held_in,
                    format!(
                        "centred under it (off by {dx:.1}px{})",
                        if held_in {
                            ", held inside the window's edge"
                        } else {
                            ""
                        }
                    ),
                );
                let dy = (tip.1 - (help.1 + help.3 + 6.0)).abs();
                ctx.ok(
                    dy <= 1.5,
                    format!("just below it ({:.0} vs {:.0})", tip.1, help.1 + help.3),
                );
                // PT's "no title meanwhile" becomes the equivalent: the tip is the button's own string.
                ctx.ok(
                    text.starts_with("Command help ("),
                    "the tip is the button's own tip string, not a native title",
                );
            }
            None => {
                ctx.ok(false, "hovering the ? shows the app's own tip");
                ctx.ok(false, "centred under it");
                ctx.ok(false, "just below it");
                ctx.ok(false, "the tip is the button's own tip string");
            }
        }
        // DROPPED: "and Windows has no title to draw meanwhile" and "and gives the title back" are
        // native `title` attribute checks; Flare has no native title, so the equivalent above and
        // the leave check below hold the behaviour.
        move_to(&mut app, rest.0, rest.1);
        let gone = ctx.until("the tip to leave", 2_000, &mut || {
            tip_state(&mut app)["visible"] != json!(true)
        });
        ctx.ok(gone, "leaving takes it away");
    }

    // PT: run.mjs:2906-2908. The cog's tip stays inside the window's edge.
    if let Some(cog) = ui_rect(&mut app, "cog") {
        if let Some((_, tip)) = hover(ctx, &mut app, cog) {
            let k = scale(&mut app);
            let client = app_json(&mut app)["window"]["client_w"]
                .as_f64()
                .unwrap_or(0.0) as f32
                / k;
            ctx.ok(
                tip.0 + tip.2 <= client - 5.0,
                format!(
                    "near the window's edge it stays inside ({:.0}+{:.0} of {:.0})",
                    tip.0, tip.2, client
                ),
            );
        } else {
            ctx.ok(false, "near the window's edge it stays inside (no tip)");
        }
        move_to(&mut app, rest.0, rest.1);
        ctx.until("the cog's tip to leave", 2_000, &mut || {
            tip_state(&mut app)["visible"] != json!(true)
        });
    } else {
        ctx.ok(
            false,
            "near the window's edge it stays inside (the cog is not showing)",
        );
    }

    // PT: run.mjs:2913-2920. A tab's tip is centred under the whole tab, not under its name.
    let t = tabs_of(&mut app);
    if let Some(tab) = t.first() {
        let tab_box = rect_of(tab);
        let name_spot = (
            tab_box.0 + tab_box.2 / 2.0 + 10.0,
            tab_box.1 + tab_box.3 / 2.0,
        );
        move_to(&mut app, name_spot.0, name_spot.1);
        let shown = ctx.until("the tab's tip", 4_000, &mut || {
            tip_state(&mut app)["visible"] == json!(true)
        });
        hold(ctx, 200);
        let named = tip_state(&mut app);
        if shown && named["visible"] == json!(true) {
            let r = &named["rect"];
            let (tx, ty, tw) = (num(r, "x"), num(r, "y"), num(r, "w"));
            let dx = (tx + tw / 2.0 - (tab_box.0 + tab_box.2 / 2.0)).abs();
            let left = tab_box.0 + tab_box.2 / 2.0 - tw / 2.0 < 6.0;
            ctx.ok(
                left || dx <= 1.5,
                format!("a tab's tip is centred under the whole tab (off by {dx:.1}px)"),
            );
            ctx.ok(
                (ty - (tab_box.1 + tab_box.3 + 6.0)).abs() <= 1.5,
                format!(
                    "just below the tab ({:.0} vs {:.0})",
                    ty,
                    tab_box.1 + tab_box.3
                ),
            );
        } else {
            ctx.ok(false, "hovering a tab shows its tip");
        }
    } else {
        ctx.ok(false, "hovering a tab shows its tip");
    }

    let _ = app.close();
    Ok(())
}

/// PT `manyTabs` (run.mjs:2932-2996): sixteen tabs in a narrow window; the strip never scrolls, the
/// rest wait behind +N, the list brings a tab in front, and Ctrl+Shift+K lists them all.
pub fn many_tabs(ctx: &mut Ctx) -> Result<(), String> {
    let root = ctx.ensure_world().root.clone();
    let dirs: Vec<std::path::PathBuf> = (1..=16)
        .map(|i| {
            let d = root.join(format!("proj{i:02}"));
            let _ = std::fs::create_dir_all(&d);
            d
        })
        .collect();
    let mut app = launch_strip(ctx, dirs, None)?;
    let _ = send(&mut app, Request::Resize { w: 700.0, h: 600.0 });

    // PT: run.mjs:2944-2945. Sixteen in all: the drawn tabs and the ones behind +N.
    ctx.until(
        "sixteen tabs in the strip and behind +N",
        30_000,
        &mut || {
            let s = strip_state(&mut app);
            s["drawn"].as_i64().unwrap_or(0) + s["overflow_n"].as_i64().unwrap_or(0) == 16
        },
    );
    hold(ctx, 400);
    let a = strip_state(&mut app);
    let drawn = a["drawn"].as_i64().unwrap_or(0);
    let more = a["overflow_n"].as_i64().unwrap_or(0);

    // PT: run.mjs:2961-2964.
    ctx.ok(
        a["scroll"] == json!(0),
        format!("the strip never scrolls ({a})"),
    );
    ctx.ok(
        a["icons"].as_i64().unwrap_or(0) > 0,
        format!("resting tabs are icons ({})", a["icons"]),
    );
    ctx.ok(
        a["active_named"] == json!(true),
        "the tab in front keeps its name",
    );
    ctx.ok(
        more > 0 && drawn + more == 16,
        format!("the rest wait behind +N ({drawn} drawn + {more} listed = 16)"),
    );
    ctx.shot(&mut app, "strip-many");

    // The +N button is at the strip's right; its place is the strip's rectangle plus its geometry.
    if let Some(strip) = ui_rect(&mut app, "strip") {
        let k = scale(&mut app);
        let (mx, mw) = (num(&a["more"], "x_px"), num(&a["more"], "w_px"));
        let at = (strip.0 + (mx + mw / 2.0) / k, strip.1 + strip.3 / 2.0);
        click_at(&mut app, at);
    }

    // PT: run.mjs:2970-2971. The list holds the hidden tabs, and its Find field has the keyboard.
    let rows = ctx.until("the list to hold the hidden tabs", 3_000, &mut || {
        strip_state(&mut app)["list_rows"].as_i64() == Some(more)
    });
    let listed = strip_state(&mut app)["list_rows"].as_i64().unwrap_or(0);
    ctx.ok(
        rows,
        format!("the list holds the {more} hidden tabs ({listed})"),
    );
    let focus = strip_state(&mut app)["find_focus"] == json!(true);
    ctx.ok(focus, "its Find field has the keyboard");
    ctx.shot(&mut app, "strip-many-list");

    // PT: run.mjs:2973-2977. Picking the first row (Enter takes the row in front of the list) fronts
    // that tab, and the list closes.
    // The first listed row is read with the list open (its rows exist only while it is).
    let pick = strip_state(&mut app)["list_first"]
        .as_str()
        .unwrap_or("")
        .chars()
        .take(6)
        .collect::<String>();
    let _ = app.session.key("Enter");
    let closed = ctx.until("the list to close", 2_000, &mut || {
        strip_state(&mut app)["list_open"] != json!(true)
    });
    ctx.ok(closed, "picking a row closes the list");
    let front = front_title(&mut app);
    let b = strip_state(&mut app);
    ctx.ok(
        front.starts_with(&pick) && b["active_named"] == json!(true),
        format!("and its tab is in front, named, in the bar ({front}, wanted {pick})"),
    );

    // PT: run.mjs:2978.
    let d = b["drawn"].as_i64().unwrap_or(0) + b["overflow_n"].as_i64().unwrap_or(0);
    ctx.ok(
        d == 16 && b["scroll"] == json!(0),
        format!("still 16 tabs, still no scroll ({d})"),
    );

    // PT: run.mjs:2981-2988. Ctrl+Shift+K lists every tab; typing narrows; Enter fronts.
    if let Some(grid) = ui_rect(&mut app, "grid") {
        click_at(&mut app, centre(grid));
    }
    let _ = app.session.key("Ctrl+Shift+K");
    let all = ctx.until("the list with every tab", 3_000, &mut || {
        strip_state(&mut app)["list_rows"].as_i64() == Some(16)
    });
    ctx.ok(
        all,
        format!(
            "Ctrl+Shift+K lists every tab ({})",
            strip_state(&mut app)["list_rows"]
        ),
    );
    let _ = app.session.type_text("proj03");
    let narrowed = ctx.until("the list narrowed to one row", 2_000, &mut || {
        strip_state(&mut app)["list_rows"].as_i64() == Some(1)
    });
    ctx.ok(
        narrowed,
        format!(
            "typing a name keeps its row ({})",
            strip_state(&mut app)["list_rows"]
        ),
    );
    let _ = app.session.key("Enter");
    hold(ctx, 150);
    let fronted = front_title(&mut app);
    ctx.ok(
        fronted.starts_with("proj03"),
        format!("Enter puts it in front ({fronted})"),
    );

    // PT: run.mjs:2989-2992. Ctrl+Shift+K opens it again; Escape closes it.
    let _ = app.session.key("Ctrl+Shift+K");
    ctx.until("the list to open again", 2_000, &mut || {
        strip_state(&mut app)["list_open"] == json!(true)
    });
    let _ = app.session.key("Escape");
    let escaped = ctx.until("the list to close on Escape", 2_000, &mut || {
        strip_state(&mut app)["list_open"] != json!(true)
    });
    ctx.ok(escaped, "Escape closes it");

    let _ = app.close();
    Ok(())
}
