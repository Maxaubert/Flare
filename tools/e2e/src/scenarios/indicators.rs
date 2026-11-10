//! Indicators scenarios (Task 25): indicator, attention, indicatorStyles.
//!
//! PT: run.mjs:522-607 (`indicator`, 12 assertions), 609-768 (`attention`, 25), 2051-2110
//! (`indicatorStyles`, 8). Prism Terminal reads the tab's DOM (`data-agent-state`, `.p-agent-run`,
//! computed colours); Flare answers through `agent.state` (presence, mark state, the taskbar badge
//! text) and `marks.state` (each tab's mark kind, motion, colour and rainbow, e2e_probes/indicators.rs).
//! Claude is stood in for by a PowerShell title (the same glyphs PT types) and a footer line.
//!
//! The Bar style, the Finished switches and the theme are driven on the real Settings page by the
//! pointer (`agent_misc::pick_seg`, `set_switch`, `driver::pick_theme`); a mark's pixels (the front
//! tab's rule, a ring's ink, a foot line's thickness and cover) are read off a snapshot
//! (`marks_px.rs` holds the maths), and a mark's pause under a minimised window comes from the probe.
//! Where Flare differs, the assertion is listed as `DROPPED:` with its reason: the tab name's
//! centring is a font fact (held on the component by `ui_tests::strip`), and the shared-bar DOM
//! count has no counterpart in a model with one mark per tab row.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use flare_e2e_proto::{PointerAction, Request};

use super::agent_misc::{goto_row, open_settings, pick_seg, set_switch, stored, switch_on};
use super::driver::pick_theme;
use super::marks_px::{best_ink, covered, dist, foot_thickness, hex_rgb, leading_run, thick_rows};
use super::strip::{
    mark_strip, picture_breathe, read_png, snap, snap_full_breath, Snap, FULL_BREATH, MARK_FOLDERS,
};
use crate::ctx::{shot_file, App, Ctx, Launch};
use crate::pixels::Rgb;
use crate::standins::agent_proc;
use crate::world::{prompt_timeout, wait_for_prompt, Shell};

/// The title PT types for an idle Claude (a black four-pointed star, then the name).
const IDLE: &str = "$Host.UI.RawUI.WindowTitle = [char]0x2733 + ' Claude Code'";
/// The title PT types for a working Claude (a half-filled circle).
const WORK: &str = "$Host.UI.RawUI.WindowTitle = [char]0x25D0 + ' Claude Code'";

fn folder(root: &Path, name: &str) -> PathBuf {
    let d = root.join(name);
    let _ = std::fs::create_dir_all(&d);
    d
}

/// A probe's answer, `Null` when it fails (a check on it then reads false).
fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// Every tab as `app.state` lists it.
fn tabs_of(app: &mut App) -> Vec<Value> {
    probe(app, "app.state", Value::Null)["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The index of the tab in front, from `app.state`.
fn front_index(app: &mut App) -> usize {
    tabs_of(app)
        .iter()
        .position(|t| t["front"] == json!(true))
        .unwrap_or(0)
}

/// `agent.state`'s tabs.
fn agents(app: &mut App) -> Vec<Value> {
    probe(app, "agent.state", Value::Null)["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `marks.state`'s tabs.
fn marks(app: &mut App) -> Vec<Value> {
    probe(app, "marks.state", Value::Null)["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Tab `i`'s agent state word (`agent.state`), `none` when it has none.
fn state(app: &mut App, i: usize) -> String {
    agents(app)
        .get(i)
        .and_then(|t| t["state"].as_str())
        .unwrap_or("none")
        .to_string()
}

/// Tab `i`'s mark record (`marks.state`), `Null` when it is missing.
fn mark(app: &mut App, i: usize) -> Value {
    marks(app).get(i).cloned().unwrap_or(Value::Null)
}

/// The taskbar badge's text as the window last told it.
fn badge(app: &mut App) -> String {
    probe(app, "agent.state", Value::Null)["badge"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// Clicks tab `i` at its centre.
fn click_tab(app: &mut App, i: usize) {
    let r = probe(app, "ui.rect", json!({ "name": format!("tab:{i}") }));
    if let (Some(x), Some(y), Some(w), Some(h)) = (
        r["x"].as_f64(),
        r["y"].as_f64(),
        r["w"].as_f64(),
        r["h"].as_f64(),
    ) {
        let _ = app
            .session
            .click((x + w / 2.0) as f32, (y + h / 2.0) as f32);
    }
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

/// Clicks tab `i` and types the line into it.
fn at(app: &mut App, i: usize, line: &str) {
    click_tab(app, i);
    type_line(app, line);
}

/// A real agent process in tab `i` (review #47): a title or a hook is a claim, and one look takes it back
/// when no agent runs in the tab. Leaves the tab inside the stand-in.
fn stand_in(ctx: &mut Ctx, app: &mut App, i: usize) {
    match agent_proc::command(&app.profile) {
        Ok(cmd) => {
            at(app, i, &cmd);
            hold(ctx, agent_proc::SETTLE_MS);
        }
        Err(e) => println!("  note  the stand-in agent could not be made ({e})"),
    }
}

/// Waits until tab `i` shows `want` (`agent.state`), up to `ms`.
fn until_state(ctx: &mut Ctx, app: &mut App, i: usize, want: &str, ms: u64) -> bool {
    ctx.until(&format!("tab {i} to show {want}"), ms, &mut || {
        state(app, i) == want
    })
}

/// Waits until the taskbar badge says `want`, up to `ms`.
fn until_badge(ctx: &mut Ctx, app: &mut App, want: &str, ms: u64) -> bool {
    ctx.until(&format!("the badge to say {want:?}"), ms, &mut || {
        badge(app) == want
    })
}

/// Holds the scenario still for `ms` in 50 ms steps.
fn hold(ctx: &mut Ctx, ms: u64) {
    for _ in 0..(ms / 50) {
        ctx.settle(50);
    }
}

/// Launches a window on `folders` at 100 percent on the software renderer.
fn open(ctx: &mut Ctx, folders: Vec<PathBuf>) -> Result<App, String> {
    let app = ctx.launch(Launch {
        renderer: Some("software"),
        folders,
        scale_dpi: Some(run_dpi()),
        ..Launch::default()
    })?;
    Ok(app)
}

// ---------------------------------------------------------------- the Bar style and the strip's pixels

/// The theme tokens the window paints with (`theme.tokens`).
fn tokens(app: &mut App) -> Value {
    probe(app, "theme.tokens", Value::Null)
}

/// A token as its three colour bytes.
fn token_rgb(tok: &Value, name: &str) -> Rgb {
    hex_rgb(tok[name].as_str().unwrap_or(""))
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

/// Tab `i`'s box in css px (`ui.rect`), `None` when the strip does not draw it.
fn tab_rect(app: &mut App, i: usize) -> Option<(f32, f32, f32, f32)> {
    let r = probe(app, "ui.rect", json!({ "name": format!("tab:{i}") }));
    Some((
        r["x"].as_f64()? as f32,
        r["y"].as_f64()? as f32,
        r["w"].as_f64()? as f32,
        r["h"].as_f64()? as f32,
    ))
}

/// Moves the pointer onto the terminal, off every tab, so no tab wears its hover ink.
fn park_pointer(app: &mut App) {
    let r = probe(app, "ui.rect", json!({ "name": "grid" }));
    let x = r["x"].as_f64().unwrap_or(400.0) + r["w"].as_f64().unwrap_or(200.0) / 2.0;
    let y = r["y"].as_f64().unwrap_or(300.0) + r["h"].as_f64().unwrap_or(200.0) / 2.0;
    let _ = app.session.call(&Request::Pointer {
        x: x as f32,
        y: y as f32,
        action: PointerAction::Move,
    });
}

/// Brings Settings to the front. The cog toggles it, so a Settings tab that is open but behind is
/// closed by the first click and opened by the second.
fn settings_forward(ctx: &mut Ctx, app: &mut App) -> bool {
    open_settings(ctx, app) || open_settings(ctx, app)
}

/// Opens Settings and picks the indicator style (the stored word: `"minimal"` is Bar, `"ring"` Ring);\n/// true when the row reads it.
fn use_style(ctx: &mut Ctx, app: &mut App, style: &str) -> bool {
    settings_forward(ctx, app) && pick_seg(ctx, app, "agent-indicator", &[style])
}

/// A snapshot decoded, taken without the scenario context (a wait's closure cannot hold it).
fn shoot(shots: &Path, scenario: &str, app: &mut App, name: &str) -> Option<Snap> {
    let path = shot_file(shots, scenario, name, |p| app.session.snapshot(p))?;
    read_png(&path)
}

/// A device pixel of the snapshot for the css point (`x`, `y`).
fn dev(css: f32, k: f32) -> u32 {
    (css * k).round().max(0.0) as u32
}

/// A tab's ground: a pixel in its left padding, before the icon.
fn ground_of(s: &Snap, k: f32, r: (f32, f32, f32, f32)) -> Rgb {
    s.at(dev(r.0 + 3.0, k), dev(r.1 + r.3 / 2.0, k))
}

/// The pixels down from a tab's top edge, in its left padding: where the front tab's rule runs.
fn rule_column(s: &Snap, k: f32, r: (f32, f32, f32, f32)) -> Vec<Rgb> {
    let x = dev(r.0 + 4.0, k);
    let y0 = dev(r.1, k);
    (0..dev(8.0, k)).map(|d| s.at(x, y0 + d)).collect()
}

/// The pixels of the row in the middle of a tab's foot bar, from its left to its right edge.
fn foot_row(s: &Snap, k: f32, r: (f32, f32, f32, f32)) -> Vec<Rgb> {
    let y = dev(r.1 + r.3, k).saturating_sub(thick_rows(1.5, k) as u32);
    (dev(r.0 + 2.0, k)..dev(r.0 + r.2 - 3.0, k))
        .map(|x| s.at(x, y))
        .collect()
}

/// The pixels up the middle of a tab's foot, from its last row.
fn foot_column(s: &Snap, k: f32, r: (f32, f32, f32, f32)) -> Vec<Rgb> {
    let x = dev(r.0 + r.2 / 2.0, k);
    let bottom = dev(r.1 + r.3, k).saturating_sub(1);
    (0..dev(10.0, k)).map(|d| s.at(x, bottom - d)).collect()
}

/// The pixels of the box at a tab's end where its ring (or its icon-tab disc) sits.
fn end_box(s: &Snap, k: f32, r: (f32, f32, f32, f32)) -> Vec<Rgb> {
    let (x0, x1) = (dev(r.0 + r.2 - 30.0, k), dev(r.0 + r.2 - 1.0, k));
    let (y0, y1) = (dev(r.1 + 2.0, k), dev(r.1 + r.3 - 4.0, k));
    let mut out = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            out.push(s.at(x, y));
        }
    }
    out
}

/// How far off the ground-to-ink line a ring's pixel may sit, and how far along it the best one must
/// reach: an antialiased stroke of the ink is on the line at any coverage.
const RING_TOL: f32 = 30.0;
const RING_MIN_T: f32 = 0.3;

/// The launch folders PT's `markedFolders` opens: six, five of them marked.
fn marked_folders(root: &Path) -> Vec<PathBuf> {
    MARK_FOLDERS.iter().map(|n| folder(root, n)).collect()
}

/// The scale the scenarios run at: 100 percent (96 dpi). The pixel maths takes the scale from the
/// window, so each check also holds at 225 and 300 percent (held by hand at 216 and 288 dpi).
fn run_dpi() -> u32 {
    96
}

/// Launches the six marked folders at `dpi`, 1400 css px wide so every tab keeps its name, with the
/// reduced-motion switch when asked, and brings the six tabs to their mockup states.
fn launch_marked(ctx: &mut Ctx, still: bool) -> Result<App, String> {
    let (root, profile) = {
        let w = ctx.ensure_world();
        (w.root.clone(), w.profile.clone())
    };
    // A second launch on the same profile would restore the first one's tabs beside the new six.
    let _ = std::fs::remove_file(profile.join("tabs.json"));
    let env = if still {
        vec![("FLARE_E2E_REDUCED_MOTION".to_string(), "1".to_string())]
    } else {
        Vec::new()
    };
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders: marked_folders(&root),
        scale_dpi: Some(run_dpi()),
        env,
        ..Launch::default()
    })?;
    let _ = app.session.call(&Request::Resize {
        w: 1400.0,
        h: 700.0,
    });
    let (settled, got) = mark_strip(ctx, &mut app);
    ctx.ok(
        settled,
        format!("six tabs in the mockups' states ({got:?})"),
    );
    Ok(app)
}

/// How far along ground to `ink` the best pixel of a tab's ring goes (1.0 is the ink itself), `None`
/// when no pixel of the box lies on that line, as a ring drawn in another colour would not.
fn ring_ink_fit(s: &Snap, k: f32, r: (f32, f32, f32, f32), ink: Rgb) -> Option<f32> {
    best_ink(end_box(s, k, r), ground_of(s, k, r), ink, RING_TOL)
}

/// `indicator`: a working title lights the tab; Ring is the mark; Bar is the other choice and wears
/// the theme's accent; a background answer leaves the finished mark, and visiting the tab clears it.
pub fn indicator(ctx: &mut Ctx) -> Result<(), String> {
    let root = ctx.ensure_world().root.clone();
    let mut app = open(ctx, vec![folder(&root, "alpha"), folder(&root, "beta")])?;
    let two = ctx.until("two tabs", 10000, &mut || tabs_of(&mut app).len() == 2);
    ctx.ok(two, "two launch folders, two tabs");
    for i in 0..2 {
        stand_in(ctx, &mut app, i);
    }
    let front = front_index(&mut app);
    // Idle first: a spinner before the agent's first rest is it starting up, not working.
    type_line(&mut app, IDLE);
    let present = ctx.until("the idle agent to be present", 8000, &mut || {
        agents(&mut app)
            .get(front)
            .is_some_and(|t| t["present"] == json!(true))
    });
    ctx.ok(present, "an idle Claude title is the agent being present");
    type_line(&mut app, WORK);
    let lit = until_state(ctx, &mut app, front, "working", 8000);
    ctx.ok(lit, "a working title lights the tab");
    // The indicator is Ring out of the box.
    let kind = mark(&mut app, front)["kind"].clone();
    ctx.ok(
        kind == json!("ring"),
        format!("the indicator is Ring out of the box ({kind})"),
    );

    // The Agents page offers Ring and Bar; Bar is stored as `minimal`.
    let there = open_settings(ctx, &mut app) && goto_row(ctx, &mut app, "agent-indicator");
    let names: Vec<String> = probe(&mut app, "settings.row", json!({ "id": "agent-indicator" }))
        ["options"]
        .as_array()
        .map(|o| {
            o.iter()
                .map(|n| n.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    ctx.ok(
        there && names.join("|") == "Ring|Bar",
        format!("the indicator offers Ring and Bar ({})", names.join("|")),
    );
    let picked = pick_seg(ctx, &mut app, "agent-indicator", &["minimal"]);
    click_tab(&mut app, 0);
    hold(ctx, 200);
    // The preference is written after a short delay.
    ctx.poll(5000, &mut || {
        stored(&app, "flare.term.agentIndicator") == "minimal"
    });
    let saved = stored(&app, "flare.term.agentIndicator");
    ctx.ok(
        picked && saved == "minimal",
        format!("Bar is stored as minimal ({saved:?})"),
    );
    // Its line is the theme's accent: the working tab behind wears the run in it.
    let accent = hex_rgb(tokens(&mut app)["accent"].as_str().unwrap_or(""));
    let run = mark(&mut app, front);
    let colour = hex_rgb(run["colour"].as_str().unwrap_or(""));
    ctx.ok(
        run["kind"] == json!("run") && colour == accent,
        format!(
            "and its line is the theme's accent ({:?} vs {accent:?}, {})",
            colour, run["kind"]
        ),
    );
    // PT: no brain icon inside a working tab (#143). Flare's tab icons are the prompt, Clawd, the
    // OpenAI mark and the cog; none reports a brain.
    let brain = tabs_of(&mut app)
        .iter()
        .any(|t| t["kind"] == json!("brain"));
    ctx.ok(!brain, "no brain icon inside a working tab any more (#143)");
    // Bar on the tab in front too: its run.
    click_tab(&mut app, front);
    let shows = ctx.until("the tab in front to show the run", 8000, &mut || {
        front_index(&mut app) == front && mark(&mut app, front)["kind"] == json!("run")
    });
    ctx.ok(shows, "the tab in front shows the run");

    // Walk away, then let it finish behind our back: the agent on tab 1 is in the background once
    // tab 0 is in front.
    click_tab(&mut app, 0);
    hold(ctx, 300);
    click_tab(&mut app, 1);
    type_line(
        &mut app,
        "Start-Sleep -Milliseconds 1500; $Host.UI.RawUI.WindowTitle = [char]0x2733 + ' Claude Code'",
    );
    click_tab(&mut app, 0);
    let done = until_state(ctx, &mut app, 1, "finished", 10000);
    ctx.ok(
        done,
        "an answer that lands on a background tab leaves the finished mark",
    );
    let working = state(&mut app, 1) == "working";
    ctx.ok(!working, "and the working mark is gone");
    click_tab(&mut app, 1);
    let cleared = until_state(ctx, &mut app, 1, "none", 5000);
    ctx.ok(cleared, "visiting the tab clears it");
    app.close();
    Ok(())
}

/// `attention`: the working marks of three neighbours; a question on a background tab marks it and
/// counts on the taskbar; answering takes it down; an agent that finishes in the background is
/// counted until its tab is opened, in a flowing rainbow until that is switched off.
pub fn attention(ctx: &mut Ctx) -> Result<(), String> {
    let root = ctx.ensure_world().root.clone();
    let mut app = open(
        ctx,
        vec![
            folder(&root, "alpha"),
            folder(&root, "beta"),
            folder(&root, "gamma"),
            folder(&root, "x"),
        ],
    )?;
    let four = ctx.until("four tabs", 10000, &mut || tabs_of(&mut app).len() == 4);
    ctx.ok(four, "four launch folders, four tabs");
    // The Bar style: PT's `useBar`.
    let bar = use_style(ctx, &mut app, "minimal");
    ctx.ok(bar, "the Bar style is on");
    click_tab(&mut app, 0);
    // DROPPED: "tab names are centred" and "a one-letter tab is at least four characters wide": the
    // tab label's text alignment and width are font facts; `ui_tests::strip` holds them on the
    // component.
    ctx.skip("tab name centring is a font fact (ui_tests::strip holds it on the component)");

    // Three Claudes side by side, all working (each a real agent process, so no claim is taken back).
    for i in 0..4 {
        stand_in(ctx, &mut app, i);
    }
    for i in 0..3 {
        click_tab(&mut app, i);
        type_line(&mut app, IDLE);
    }
    for i in 0..3 {
        at(&mut app, i, WORK);
    }
    let all = ctx.until("three working tabs", 8000, &mut || {
        (0..3).all(|i| state(&mut app, i) == "working")
    });
    ctx.ok(all, "three working tabs");
    // ONE BAR PER TAB: each working neighbour draws its own run (PT counted `.p-agent-run` per tab).
    let runs: Vec<String> = (0..3)
        .map(|i| mark(&mut app, i)["kind"].as_str().unwrap_or("").to_string())
        .collect();
    ctx.ok(
        runs.join(",") == "run,run,run",
        format!(
            "three working neighbours each draw their own bar ({})",
            runs.join(",")
        ),
    );
    // DROPPED: "and no shared bar across them": the model has one mark per tab row, so there is no
    // shared element to count; the three runs above are the three rows' own.
    at(&mut app, 1, IDLE);
    let kept = ctx.until("the other two to keep theirs", 5000, &mut || {
        let runs = |app: &mut App| {
            (0..3)
                .filter(|&i| mark(app, i)["kind"] == json!("run") && state(app, i) == "working")
                .count()
        };
        runs(&mut app) == 2
    });
    ctx.ok(kept, "with the middle one idle, the other two keep theirs");

    // A QUESTION on a background tab: Claude's footer on screen, title idle.
    at(
        &mut app,
        0,
        &format!(
            "Start-Sleep -Milliseconds 1500; Write-Host 'Enter to select, Esc to cancel'; {IDLE}"
        ),
    );
    click_tab(&mut app, 1);
    let asked = until_state(ctx, &mut app, 0, "question", 10000);
    ctx.ok(asked, "a question asked on a background tab marks it");
    let k = scale(&mut app);
    park_pointer(&mut app);
    hold(ctx, 200);
    let line = |ctx: &mut Ctx, app: &mut App, name: &str, i: usize| -> (usize, f32) {
        let (Some(r), Some(s)) = (tab_rect(app, i), snap(ctx, app, name)) else {
            return (0, 0.0);
        };
        let ground = ground_of(&s, k, r);
        (
            foot_thickness(foot_column(&s, k, r), ground, 12),
            covered(&foot_row(&s, k, r), ground, 12),
        )
    };
    // A question's line breathes: a picture at its faintest shows no line, so it is looked at over a breath (up
    // to six pictures, 250 ms apart) and the clearest counts (the Mac runner caught it at 0 px once; rule 1119).
    // Windows looks once, as before.
    let want = thick_rows(3.0, k);
    let tries = if cfg!(target_os = "macos") { 6 } else { 1 };
    let mut best = (0usize, 0.0f32);
    for attempt in 0..tries {
        let got = line(ctx, &mut app, "question", 0);
        if (got.0 == want && got.1 > best.1) || (best.0 != want && got.0 > best.0) {
            best = got;
        }
        if (best.0 == want && best.1 > 0.9) || attempt + 1 == tries {
            break;
        }
        hold(ctx, 250);
    }
    let (thick, share) = best;
    let m0 = mark(&mut app, 0);
    ctx.ok(
        m0["kind"] == json!("line") && thick == want && share > 0.9,
        format!(
            "as a line along the bottom (kind {}, {thick} px thick, {:.0}% of the foot)",
            m0["kind"],
            share * 100.0
        ),
    );
    ctx.ok(
        m0["motion"] == json!("breathe"),
        format!("that breathes ({})", m0["motion"]),
    );
    let counted = until_badge(ctx, &mut app, "1 tab needs a look", 4000);
    ctx.ok(
        counted,
        format!("and the taskbar badge counts it ({})", badge(&mut app)),
    );
    // AS CRISP AS THE OTHER APPS': the badge is drawn at the display's physical size.
    let want = (16.0 * k.clamp(1.0, 4.0)).round() as u64;
    let have = probe(&mut app, "agent.state", Value::Null)["badge_px"]
        .as_u64()
        .unwrap_or(0);
    ctx.ok(
        have == want,
        format!("the badge is {want}px at scale {k}, not a stretched small image ({have}px)"),
    );

    // A QUESTION LASTS UNTIL IT IS ANSWERED: opening the tab does not clear it.
    click_tab(&mut app, 0);
    park_pointer(&mut app);
    hold(ctx, 400);
    ctx.ok(
        state(&mut app, 0) == "question",
        format!(
            "opening the tab does not clear the question ({})",
            state(&mut app, 0)
        ),
    );
    let on_front = front_index(&mut app) == 0;
    let (thick, share) = line(ctx, &mut app, "question-active", 0);
    ctx.ok(
        on_front && mark(&mut app, 0)["kind"] == json!("line"),
        "its line is drawn on the tab in front",
    );
    ctx.ok(
        thick == thick_rows(3.0, k) && share > 0.9,
        format!(
            "and it is visible there ({thick} px thick, {:.0}% of the foot)",
            share * 100.0
        ),
    );
    ctx.ok(
        badge(&mut app) == "1 tab needs a look",
        format!("and the badge still counts it ({})", badge(&mut app)),
    );
    click_tab(&mut app, 1);
    hold(ctx, 600);
    ctx.ok(
        state(&mut app, 0) == "question",
        "leaving it unanswered keeps the line",
    );
    // Answering is work again: the line goes, and the badge with it.
    at(
        &mut app,
        0,
        &format!("{}; {WORK}", crate::world::CLEAR_HOST),
    );
    let answered = until_state(ctx, &mut app, 0, "working", 8000);
    ctx.ok(answered, "answering it takes the question down");
    let unbadged = until_badge(ctx, &mut app, "", 4000);
    ctx.ok(unbadged, "and the badge with it");
    at(&mut app, 0, IDLE);
    hold(ctx, 800);
    ctx.ok(
        state(&mut app, 0) == "none",
        format!("and it does not come back at rest ({})", state(&mut app, 0)),
    );

    // FINISHED on a background tab: counted until the tab is opened.
    at(
        &mut app,
        2,
        &format!("Start-Sleep -Milliseconds 1500; {IDLE}"),
    );
    click_tab(&mut app, 1);
    let finished = until_state(ctx, &mut app, 2, "finished", 10000);
    ctx.ok(
        finished,
        "an agent that finishes on a background tab marks it",
    );
    let counted = until_badge(ctx, &mut app, "1 tab needs a look", 4000);
    ctx.ok(counted, "and is counted on the taskbar");
    // THE RAINBOW FINISH: the icon's colours, flowing, on by default.
    let rb = mark(&mut app, 2);
    ctx.ok(
        rb["kind"] == json!("line")
            && rb["rainbow"] == json!(true)
            && rb["motion"] == json!("flow"),
        format!("the finished line is the flowing rainbow ({rb})"),
    );
    let shown = settings_forward(ctx, &mut app);
    let there = shown && goto_row(ctx, &mut app, "agent-rainbow");
    ctx.ok(
        there && switch_on(&mut app, "agent-rainbow") == Some(true),
        "Rainbow finished mark is on by default",
    );
    let off = set_switch(ctx, &mut app, "agent-rainbow", false);
    let plain = ctx.until("the finished line to go plain", 4000, &mut || {
        let m = mark(&mut app, 2);
        m["rainbow"] == json!(false) && m["motion"] == json!("none")
    });
    ctx.ok(
        off && plain,
        format!(
            "switched off, the finished line is the finished colour, still ({})",
            mark(&mut app, 2)
        ),
    );
    set_switch(ctx, &mut app, "agent-rainbow", true);
    // Switched off, the finished mark goes, and the badge has nothing to count.
    settings_forward(ctx, &mut app);
    let done_off = set_switch(ctx, &mut app, "agent-done-on", false);
    let nothing = until_badge(ctx, &mut app, "", 4000);
    ctx.ok(
        done_off && nothing,
        format!(
            "with the Finished indicator off, nothing is counted ({:?})",
            badge(&mut app)
        ),
    );
    let drawn = marks(&mut app)
        .iter()
        .any(|m| m["state"] == json!("finished"));
    ctx.ok(!drawn, "and no finished line is drawn");
    set_switch(ctx, &mut app, "agent-done-on", true);
    at(&mut app, 0, "$Host.UI.RawUI.WindowTitle = 'pwsh'");
    app.close();
    Ok(())
}

/// `indicatorStyles`: on Volt and Paper, Ring's mark at each marked tab's end in its name's ink, or
/// Bar's line along the foot; the tab in front is told apart in every state; the rainbow flows, and
/// under reduced motion nothing moves at all; a minimised window holds the marks.
pub fn indicator_styles(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = launch_marked(ctx, false)?;
    let marked = [0usize, 2, 3, 4, 5];
    for theme in ["volt", "paper"] {
        let themed = pick_theme(ctx, &mut app, theme);
        ctx.ok(themed, format!("{theme} is the theme in force"));
        for style in ["minimal", "ring"] {
            let picked = use_style(ctx, &mut app, style);
            click_tab(&mut app, 0);
            park_pointer(&mut app);
            hold(ctx, 300);
            let tag = format!("{theme}, {style}");
            let k = scale(&mut app);
            let tok = tokens(&mut app);
            let rule = token_rgb(&tok, "accent_hi");
            let (text, dim) = (token_rgb(&tok, "text"), token_rgb(&tok, "dim"));
            let kinds: Vec<String> = marks(&mut app)
                .iter()
                .map(|m| m["kind"].as_str().unwrap_or("").to_string())
                .collect();
            let Some(s) = snap(ctx, &mut app, &format!("marks-{style}-{theme}")) else {
                ctx.ok(false, format!("{tag}: a snapshot of the strip"));
                continue;
            };
            let rects: Vec<_> = (0..6).map(|i| tab_rect(&mut app, i)).collect();
            ctx.ok(
                picked && rects.iter().all(Option::is_some),
                format!("{tag}: the style is picked and the six tabs are drawn"),
            );
            let rects: Vec<(f32, f32, f32, f32)> = rects.into_iter().flatten().collect();
            if rects.len() < 6 {
                continue;
            }
            let rows = leading_run(rule_column(&s, k, rects[0]), rule, 14);
            let others = (1..6)
                .filter(|&i| leading_run(rule_column(&s, k, rects[i]), rule, 14) > 0)
                .count();
            ctx.ok(
                front_index(&mut app) == 0 && rows.abs_diff(thick_rows(2.0, k)) <= 1 && others == 0,
                format!(
                    "{tag}: the tab in front wears its rule ({rows} px, others {others}, want {})",
                    thick_rows(2.0, k)
                ),
            );
            if style == "ring" {
                let alone = marked
                    .iter()
                    .all(|&i| kinds.get(i).map(String::as_str) == Some("ring"));
                ctx.ok(
                    alone,
                    format!("{tag}: every marked tab wears Ring's mark alone ({kinds:?})"),
                );
                // The question tab's dot breathes from full to nothing: its ink is read in a picture
                // the app says it drew at full breath, not whichever moment the first one caught.
                let full = snap_full_breath(ctx, &mut app, &format!("marks-{style}-{theme}-full"));
                let breath = picture_breathe(&mut app);
                ctx.ok(
                    full.is_some() && breath.is_some_and(|b| b >= FULL_BREATH),
                    format!("{tag}: a picture of the question dot at full breath ({breath:?})"),
                );
                let fits: Vec<Option<f32>> = match &full {
                    Some(f) => marked
                        .iter()
                        .map(|&i| ring_ink_fit(f, k, rects[i], if i == 0 { text } else { dim }))
                        .collect(),
                    None => vec![None; marked.len()],
                };
                ctx.ok(
                    fits.iter().all(|f| f.is_some_and(|t| t >= RING_MIN_T)),
                    format!("{tag}: every ring wears its name's ink ({fits:?})"),
                );
            } else {
                let mut lines: Vec<f32> = [2usize, 3, 4, 5]
                    .iter()
                    .map(|&i| {
                        let g = ground_of(&s, k, rects[i]);
                        covered(&foot_row(&s, k, rects[i]), g, 12)
                    })
                    .collect();
                // A run travels and a question line breathes: one that is not in the first picture is
                // waited for, since a moment of it off the tab is the motion, not a missing line.
                let (shots, scenario) = (ctx.shots.clone(), ctx.name);
                for (n, &i) in [2usize, 3, 4, 5].iter().enumerate() {
                    if lines[n] > 0.0 {
                        continue;
                    }
                    let mut share = 0.0;
                    ctx.until(&format!("tab {i}'s line to show"), 3000, &mut || {
                        let r = tab_rect(&mut app, i);
                        let s2 = shoot(&shots, scenario, &mut app, "marks-retry");
                        let (Some(r), Some(s2)) = (r, s2) else {
                            return false;
                        };
                        share = covered(&foot_row(&s2, k, r), ground_of(&s2, k, r), 12);
                        share > 0.0
                    });
                    lines[n] = share;
                }
                let drawn = [2usize, 3, 4, 5].iter().zip(&lines).all(|(&i, share)| {
                    matches!(kinds.get(i).map(String::as_str), Some("run" | "line")) && *share > 0.0
                });
                ctx.ok(
                    drawn,
                    format!("{tag}: a line along every marked tab ({kinds:?}, {lines:?})"),
                );
            }
        }
    }
    // Bar, back on the first tab: the rainbow and the pause are looked at in the style that draws them.
    let bar = use_style(ctx, &mut app, "minimal");
    click_tab(&mut app, 0);
    park_pointer(&mut app);
    hold(ctx, 300);
    ctx.ok(bar, "Bar is the style for the motion checks");
    // THE RAINBOW FLOWS: two samples of the finished line differ.
    let k = scale(&mut app);
    let foot = |ctx: &mut Ctx, app: &mut App, name: &str| -> Vec<Rgb> {
        match (tab_rect(app, 3), snap(ctx, app, name)) {
            (Some(r), Some(s)) => foot_row(&s, k, r),
            _ => Vec::new(),
        }
    };
    let f1 = foot(ctx, &mut app, "rainbow-1");
    hold(ctx, 400);
    let f2 = foot(ctx, &mut app, "rainbow-2");
    let m3 = mark(&mut app, 3);
    ctx.ok(
        !f1.is_empty() && f1 != f2 && m3["motion"] == json!("flow"),
        format!(
            "the rainbow flows ({} px sampled, motion {})",
            f1.len(),
            m3["motion"]
        ),
    );
    // MINIMISE: the marks are held where they stood, and go on after a restore.
    let moving: Vec<usize> = marks(&mut app)
        .iter()
        .enumerate()
        .filter(|(_, m)| !matches!(m["motion"].as_str(), Some("none" | "still") | None))
        .map(|(i, _)| i)
        .collect();
    let _ = app.session.call(&Request::Window {
        op: "minimise".into(),
    });
    let held = ctx.until("the moving marks to be held", 6000, &mut || {
        !moving.is_empty()
            && moving
                .iter()
                .all(|&i| mark(&mut app, i)["paused"] == json!(true))
    });
    ctx.ok(held, format!("minimising pauses the marks ({moving:?})"));
    let _ = app.session.call(&Request::Window {
        op: "restore".into(),
    });
    let freed = ctx.until("the marks to go on", 6000, &mut || {
        moving
            .iter()
            .all(|&i| mark(&mut app, i)["paused"] == json!(false))
    });
    ctx.ok(freed, "and restoring sets them going again");
    app.close();

    // REDUCED MOTION: everything still. The switch is read at start, so the app is started again.
    let mut app = launch_marked(ctx, true)?;
    let bar = use_style(ctx, &mut app, "minimal");
    click_tab(&mut app, 0);
    park_pointer(&mut app);
    hold(ctx, 300);
    ctx.ok(bar, "Bar is on under reduced motion");
    let motions: Vec<String> = marks(&mut app)
        .iter()
        .map(|m| m["motion"].as_str().unwrap_or("").to_string())
        .collect();
    let moving = motions
        .iter()
        .any(|m| !matches!(m.as_str(), "none" | "still"));
    ctx.ok(
        !moving && motions.iter().any(|m| m == "still"),
        format!("with reduced motion no mark moves ({motions:?})"),
    );
    let k = scale(&mut app);
    let foot = |ctx: &mut Ctx, app: &mut App, name: &str, i: usize| -> (Vec<Rgb>, Rgb) {
        match (tab_rect(app, i), snap(ctx, app, name)) {
            (Some(r), Some(s)) => (foot_row(&s, k, r), ground_of(&s, k, r)),
            _ => (Vec::new(), [0; 3]),
        }
    };
    let (q1, _) = foot(ctx, &mut app, "still-1", 3);
    hold(ctx, 400);
    let (q2, _) = foot(ctx, &mut app, "still-2", 3);
    ctx.ok(!q1.is_empty() && q1 == q2, "the rainbow stands still");
    let (run, ground) = foot(ctx, &mut app, "still-run", 2);
    let (question, qground) = foot(ctx, &mut app, "still-question", 4);
    let qcolour = hex_rgb(mark(&mut app, 4)["colour"].as_str().unwrap_or(""));
    let full = covered(&run, ground, 12);
    let on = dist(
        question.get(question.len() / 2).copied().unwrap_or(qground),
        qcolour,
    );
    ctx.ok(
        full > 0.95 && on <= 12,
        format!(
            "the run is a full line and the question fully on ({:.0}% of the foot, {on} off its colour)",
            full * 100.0
        ),
    );
    app.close();
    Ok(())
}
