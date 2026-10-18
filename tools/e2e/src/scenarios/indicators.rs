//! Indicators scenarios (Task 25): indicator, attention, indicatorStyles.
//!
//! PT: run.mjs:522-607 (`indicator`, 12 assertions), 609-768 (`attention`, 25), 2051-2110
//! (`indicatorStyles`, 8). Prism Terminal reads the tab's DOM (`data-agent-state`, `.p-agent-run`,
//! computed colours); Flare answers through `agent.state` (presence, mark state, the taskbar badge
//! text) and `marks.state` (each tab's mark kind, motion, colour and rainbow, e2e_probes/indicators.rs).
//! Claude is stood in for by a PowerShell title (the same glyphs PT types) and a footer line.
//!
//! Where Flare differs or the check cannot be made from a probe, the assertion is listed as
//! `DROPPED:` with its reason: the Bar style and the Finished rainbow are set through the Agents
//! pages (no probe picks a settings segment yet); the tab label geometry, the line's pixel height
//! and colour, the taskbar badge image and the shared-bar DOM count are pixel or DOM facts.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ctx::{App, Ctx, Launch};
use crate::world::{Shell, prompt_timeout, wait_for_prompt};

/// The title PT types for an idle Claude (a black four-pointed star, then the name).
const IDLE: &str = "$Host.UI.RawUI.WindowTitle = [char]0x2733 + ' Claude Code'";
/// The title PT types for a working Claude (a half-filled circle).
const WORK: &str = "$Host.UI.RawUI.WindowTitle = [char]0x25D0 + ' Claude Code'";

/// The reason the Bar-style checks cannot run yet.
const BAR_SKIP: &str =
    "the Bar style is set on the Agents page; no probe picks its segment yet (Task 33)";

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
        scale_dpi: Some(96),
        ..Launch::default()
    })?;
    Ok(app)
}

/// `indicator`: a working title lights the tab; Ring is the mark; a background answer leaves the
/// finished mark, and visiting the tab clears it.
pub fn indicator(ctx: &mut Ctx) -> Result<(), String> {
    let root = ctx.ensure_world().root.clone();
    let mut app = open(ctx, vec![folder(&root, "alpha"), folder(&root, "beta")])?;
    let two = ctx.until("two tabs", 10000, &mut || tabs_of(&mut app).len() == 2);
    ctx.ok(two, "two launch folders, two tabs");
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
    // DROPPED: "the indicator offers Ring and Bar" (the Agents page's segment picker; see BAR_SKIP).
    // DROPPED: "Bar is stored as minimal", "its line is the theme's accent" and "the tab in front
    // shows the run" (the Bar style; BAR_SKIP).
    // DROPPED: "no brain icon inside a working tab (#143)": the tab's icon kind is a field of
    // `app.state` (`kind`), and no tab reports "brain" to check against.
    ctx.skip(BAR_SKIP);

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
/// counted until its tab is opened.
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
    // DROPPED: "tab names are centred" and "a one-letter tab is at least four characters wide": the
    // tab label's text alignment and width are pixel and font facts, not probe facts.
    ctx.skip("tab label geometry is a pixel fact (no probe)");

    // Three Claudes side by side, all working.
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
    // ONE MARK PER TAB: each working neighbour carries its own mark (PT counted its bars in the DOM).
    let own = (0..3).all(|i| {
        let m = mark(&mut app, i);
        m["state"] == json!("working") && m["kind"] != json!("none")
    });
    ctx.ok(own, "three working neighbours each carry their own mark");
    // DROPPED: "and no shared bar across them": the model has one mark per tab row, so there is no
    // shared element to count.
    at(&mut app, 1, IDLE);
    let kept = ctx.until("the other two to keep theirs", 5000, &mut || {
        state(&mut app, 0) == "working" && state(&mut app, 2) == "working"
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
    // DROPPED: "as a line along the bottom" (the 3 px line is the Bar style, BAR_SKIP; Ring draws a
    // 7 px dot, and the line's pixel height is not probed).
    let m0 = mark(&mut app, 0);
    ctx.ok(
        m0["motion"] == json!("breathe"),
        format!("that breathes ({})", m0["motion"]),
    );
    let counted = until_badge(ctx, &mut app, "1 tab needs a look", 4000);
    ctx.ok(
        counted,
        format!("and the taskbar badge counts it ({})", badge(&mut app)),
    );
    // DROPPED: the badge image is 16 px at the display's scale (the taskbar picture is not probed).
    ctx.skip("the taskbar badge picture is not probed");

    // A QUESTION LASTS UNTIL IT IS ANSWERED: opening the tab does not clear it.
    click_tab(&mut app, 0);
    hold(ctx, 400);
    ctx.ok(
        state(&mut app, 0) == "question",
        format!(
            "opening the tab does not clear the question ({})",
            state(&mut app, 0)
        ),
    );
    // DROPPED: "its line is drawn on the tab in front" and "visible there" (the line's pixels and
    // colour, not probed).
    click_tab(&mut app, 1);
    hold(ctx, 600);
    ctx.ok(
        state(&mut app, 0) == "question",
        "leaving it unanswered keeps the line",
    );
    // Answering is work again: the line goes, and the badge with it.
    at(&mut app, 0, &format!("Clear-Host; {WORK}"));
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
    // DROPPED: "the finished line is the flowing rainbow" and "switched off, the finished line is
    // plain": the rainbow is a Bar-style look (BAR_SKIP), and the Finished switch is on the Agents
    // page (Task 33).
    ctx.skip(BAR_SKIP);
    // DROPPED: "with the Finished indicator off, nothing is counted" and "no finished line is
    // drawn": the Finished switch is an Agents-page toggle (Task 33 owns that page's probe).
    ctx.skip("the Finished switch needs the Agents page (Task 33)");
    app.close();
    Ok(())
}

/// `indicatorStyles`: each style's front-tab rule, Ring's mark on every marked tab in the name's ink,
/// the Bar line per marked tab, the flowing rainbow, reduced motion, and minimise pausing. The
/// assertions read the theme and style through the Agents and Appearance pages and the pixels of the
/// tab strip; none is reachable from a probe yet.
pub fn indicator_styles(ctx: &mut Ctx) -> Result<(), String> {
    // DROPPED: "<theme>, <style>: the tab in front wears its rule" (pixel rule, 2 px), "every marked
    // tab wears Ring's mark alone" and "every ring wears its name's ink" (the ring's colour against
    // the name's ink, pixels), "a line along every marked tab" (Bar, BAR_SKIP): all need the
    // Appearance theme wall and the Agents style, and the pixels of the strip.
    // DROPPED: "the rainbow flows" (two samples of the finished line's position), "reduced motion
    // stills it" (the marks.state motion check is ported in the unit tests of e2e_probes/indicators.rs
    // as `reduced_motion_stills_every_moving_mark`), and "minimise pauses" (no probe reads the mark
    // clock's pause).
    ctx.skip("the styles need the Appearance and Agents pages and the strip's pixels");
    Ok(())
}
