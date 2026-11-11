//! Agent odds and ends scenarios (Task 27): agentDiag, backgroundWork, agentKeys.
//!
//! PT: run.mjs:999-1081 (agentDiag, 12 checks), 3005-3032 (backgroundWork, 3), 3057-3120 (agentKeys, 10).
//! A shell stands in for Claude: it prints the real hook lines (`hook_line`) and sets the console title,
//! as Claude does. The diagnostics log is read with M9's reader (`flare_diag::reader::read_lines`); the
//! leak check reads every file of the log folder, rotated ones too. The hotkey and sidebar checks read the
//! saved preferences in `settings.json` (PT reads `localStorage`), as `vertical.rs` does.
//! PT's bare `until` (no check of its own) is `soft`: it waits without recording a FAIL.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flare_diag::reader::{read_lines, Line};
use flare_e2e_proto::{PointerAction, Request};
use serde_json::{json, Value};

use crate::ctx::{App, Ctx, Launch};
use crate::standins::agent_proc;
use crate::standins::hooklines::hook_line;
use crate::until::{wait_on, SystemClock};
use crate::world::{prompt_timeout, wait_for_agent_verdict, wait_for_prompt, Shell};

/// The marker the screen and the title carry: it must never reach the log.
const SECRET: &str = "SECRET-7Q";

/// The mode line's second half of a footer with a shell running (PT's ` · 1 shell`), as PowerShell.
const SHELL_FOOTER: &str = " + [char]0x00B7 + ' 1 shell'";

/// How long a process poll may take to say its first word on a shell.
const VERDICT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------- probes and the screen

/// A probe's answer, `Null` when it fails (a check on it then reads false).
fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// The `agent.state` answer.
fn agent(app: &mut App) -> Value {
    probe(app, "agent.state", Value::Null)
}

/// How many shell tabs `agent.state` lists (Settings is not one).
fn tab_count(app: &mut App) -> usize {
    agent(app)["tabs"].as_array().map_or(0, Vec::len)
}

/// How many tabs the strip holds, Settings included (PT's `[data-tab]` count).
fn all_tab_count(app: &mut App) -> usize {
    probe(app, "app.state", Value::Null)["tabs"]
        .as_array()
        .map_or(0, Vec::len)
}

/// The id of tab `i` in strip order.
fn tab_id(app: &mut App, i: usize) -> Option<String> {
    agent(app)["tabs"][i]["id"].as_str().map(str::to_string)
}

/// The state word of tab `i`: `none`, `working`, `question`, `finished` or `failed`.
fn state(app: &mut App, i: usize) -> String {
    agent(app)["tabs"][i]["state"]
        .as_str()
        .unwrap_or("?")
        .to_string()
}

/// The index, among the shells, of the tab in front (PT's `front()`).
fn front_shell(app: &mut App) -> Option<usize> {
    let st = probe(app, "app.state", Value::Null);
    st["tabs"]
        .as_array()?
        .iter()
        .filter(|t| t["kind"] != json!("settings"))
        .position(|t| t["front"] == json!(true))
}

/// A rectangle value as a tuple.
fn boxed(v: &Value) -> Option<(f64, f64, f64, f64)> {
    Some((
        v["x"].as_f64()?,
        v["y"].as_f64()?,
        v["w"].as_f64()?,
        v["h"].as_f64()?,
    ))
}

/// The centre of the named UI rectangle (`ui.rect`), when it is drawn.
fn ui_centre(app: &mut App, name: &str) -> Option<(f32, f32)> {
    let r = probe(app, "ui.rect", json!({ "name": name }));
    boxed(&r).map(|(x, y, w, h)| ((x + w / 2.0) as f32, (y + h / 2.0) as f32))
}

/// The centre of the grid: where a click lands on the shell (PT's `.xterm` click).
fn grid_centre(app: &mut App) -> (f32, f32) {
    ui_centre(app, "grid").unwrap_or((500.0, 300.0))
}

/// Clicks the centre of the grid (the shell takes the focus).
fn click_grid(app: &mut App) {
    let (x, y) = grid_centre(app);
    let _ = app.session.click(x, y);
}

/// Clicks the strip's box of tab `i` (the box's centre).
fn tab_click(app: &mut App, i: usize) -> bool {
    let st = probe(app, "app.state", Value::Null);
    let Some((x, y, w, h)) = boxed(&st["tabs"][i]["rect"]) else {
        return false;
    };
    app.session
        .click((x + w / 2.0) as f32, (y + h / 2.0) as f32)
        .is_ok()
}

/// PT `typeLine`: waits for the prompt in tab `id`, then types `cmd` and Enter.
fn type_line(app: &mut App, id: &str, cmd: &str) {
    wait_for_prompt(
        app,
        id,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    );
    let _ = app.session.type_text(cmd);
    let _ = app.session.key("Enter");
}

/// PT `at(i, cmd)`: tab `i` to the front, then the line typed into it.
fn at(app: &mut App, i: usize, cmd: &str) {
    tab_click(app, i);
    if let Some(id) = tab_id(app, i) {
        type_line(app, &id, cmd);
    }
}

/// PT `look(i)`: the window has the focus, and tab `i` is opened.
fn look(app: &mut App, i: usize) {
    let _ = app.session.call(&Request::Focus { on: true });
    tab_click(app, i);
}

/// PT's `blur` event: nobody is looking, as a parked window is not (it can start focused).
fn blur(app: &mut App) {
    let _ = app.session.call(&Request::Focus { on: false });
}

// ---------------------------------------------------------------- the stand-in's lines

/// A PowerShell expression that writes the hook line `raw` (ESC and BEL as `[char]`s).
fn ps_write(raw: &str) -> String {
    let body = raw
        .trim_start_matches('\u{1b}')
        .trim_end_matches('\u{7}')
        .replace('\'', "''");
    format!("[Console]::Write([char]27 + '{body}' + [char]7)")
}

/// PT `say(state)`: the hook line of `state`.
fn say(state: &str) -> String {
    ps_write(&hook_line(state, &json!({})))
}

/// PT `later(cmd)`: waits a moment, so the tab can be left before the hook lands.
fn later(cmd: &str) -> String {
    format!("Start-Sleep -Milliseconds 1500; {cmd}")
}

/// The console title with the marker text in it (PT's `title(glyph)`).
fn title_line(glyph: &str) -> String {
    format!("$Host.UI.RawUI.WindowTitle = [char]0x{glyph} + ' {SECRET} fix the login'")
}

/// The footer Claude draws: a rule, the prompt, a rule, and the mode line with `extra` (a PowerShell
/// fragment) after `auto mode on`.
fn footer_line(extra: &str) -> String {
    format!(
        "$r = [string]::new([char]0x2500, 40); Clear-Host; Write-Host $r; Write-Host ([char]0x276F); Write-Host $r; Write-Host ('  auto mode on'{extra})"
    )
}

/// Waits `ms` without a FAIL (PT's `sleep`), through the one wait primitive's clock.
fn pause(ctx: &mut Ctx, ms: u64) {
    let start = Instant::now();
    ctx.until("a pause to pass", ms + 5_000, &mut || {
        start.elapsed() >= Duration::from_millis(ms)
    });
}

/// Waits up to `timeout_ms` for `f` without recording a FAIL (PT's bare `until`).
fn soft(timeout_ms: u64, f: &mut dyn FnMut() -> bool) -> bool {
    let mut clock = SystemClock::new();
    wait_on(&mut clock, timeout_ms, f)
}

// ---------------------------------------------------------------- the saved preferences

/// A saved preference as a string, `""` when it is not stored yet.
pub(super) fn stored(app: &App, key: &str) -> String {
    let text = std::fs::read_to_string(app.profile.join("settings.json")).unwrap_or_default();
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v[key].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The sidebar's saved mode, `open` when nothing is stored (PT's `?? 'open'`).
fn sidebar_mode(app: &App) -> String {
    match stored(app, "flare.window.sidebar") {
        s if s.is_empty() => "open".to_string(),
        s => s,
    }
}

// ---------------------------------------------------------------- the diagnostics log

/// The lines of the log, in time order, every file of the log folder (rotated ones too).
fn log_lines(app: &App) -> Vec<Line> {
    read_lines(&crate::world::logs_dir(&app.profile))
}

/// Every file of the log folder, as text.
fn log_texts(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|e| std::fs::read(e.path()).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .collect()
}

/// A text field of a log line.
fn field<'a>(l: &'a Line, name: &str) -> Option<&'a str> {
    l.raw.get(name)?.as_str()
}

/// The `id` of the first agent hook line in the log (PT's `hooks[0].id`), `""` when there is none.
fn first_hook_id(lines: &[Line]) -> String {
    lines
        .iter()
        .find(|l| field(l, "k") == Some("agent-hook"))
        .and_then(|l| field(l, "id"))
        .unwrap_or_default()
        .to_string()
}

/// The hook signals of tab `id`, each as its state, with `xN` when a run was folded into N more.
pub fn hook_sequence(lines: &[Line], id: &str) -> Vec<String> {
    lines
        .iter()
        .filter(|l| field(l, "k") == Some("agent-hook") && field(l, "id") == Some(id))
        .map(|l| {
            let state = field(l, "state").unwrap_or_default();
            match l.raw.get("repeats").and_then(Value::as_u64) {
                Some(n) if n > 0 => format!("{state}x{n}"),
                _ => state.to_string(),
            }
        })
        .collect()
}

/// The mark moves of tab `id`, each as `from>to: why`.
pub fn mark_sequence(lines: &[Line], id: &str) -> Vec<String> {
    marks_of(lines, id)
        .into_iter()
        .map(|l| {
            format!(
                "{}>{}: {}",
                field(l, "from").unwrap_or_default(),
                field(l, "to").unwrap_or_default(),
                field(l, "why").unwrap_or_default()
            )
        })
        .collect()
}

/// The mark lines of tab `id`.
fn marks_of<'a>(lines: &'a [Line], id: &str) -> Vec<&'a Line> {
    lines
        .iter()
        .filter(|l| field(l, "k") == Some("agent-mark") && field(l, "id") == Some(id))
        .collect()
}

/// The meanings the title of tab `id` was written with, in order.
pub fn title_sequence(lines: &[Line], id: &str) -> Vec<String> {
    lines
        .iter()
        .filter(|l| field(l, "k") == Some("agent-title") && field(l, "id") == Some(id))
        .map(|l| field(l, "state").unwrap_or_default().to_string())
        .collect()
}

/// How many lines of the log files hold `needle` (the marker must be in none).
pub fn leaking_lines(texts: &[String], needle: &str) -> usize {
    texts
        .iter()
        .flat_map(|t| t.lines())
        .filter(|l| l.contains(needle))
        .count()
}

// ---------------------------------------------------------------- the settings pane

/// The probe that answers for one settings row.
fn row(app: &mut App, id: &str) -> Value {
    probe(app, "settings.row", json!({ "id": id }))
}

/// Whether the settings pane is open (any tab of it).
fn settings_open(app: &mut App) -> bool {
    probe(app, "settings.state", Value::Null)["open"] == json!(true)
}

/// Whether the settings pane is the tab in front.
fn settings_front(app: &mut App) -> bool {
    probe(app, "settings.state", Value::Null)["front"] == json!(true)
}

/// The value word of a pill row, lowercased, and whether it is one of `want`.
pub(super) fn row_is(app: &mut App, id: &str, want: &[&str]) -> bool {
    let v = row(app, id)["value"].as_str().unwrap_or("").to_lowercase();
    want.iter().any(|w| v == *w)
}

/// The switch row `id` as on or off (`None` when the row is not drawn).
pub(super) fn switch_on(app: &mut App, id: &str) -> Option<bool> {
    row(app, id)["on"].as_bool()
}

/// Opens Settings from the cog, or brings it to the front; waits for it to be in front.
pub(super) fn open_settings(ctx: &mut Ctx, app: &mut App) -> bool {
    if settings_front(app) {
        return true;
    }
    if let Some((x, y)) = ui_centre(app, "cog") {
        let _ = app.session.click(x, y);
    }
    ctx.until("Settings is in front", 8_000, &mut || settings_front(app))
}

/// Closes Settings with the cog, which is a toggle (PT's `[data-title-settings]` click).
fn close_settings(app: &mut App) -> bool {
    if let Some((x, y)) = ui_centre(app, "cog") {
        let _ = app.session.click(x, y);
    }
    soft(4_000, &mut || !settings_open(app))
}

/// The page a settings row lives on, from the catalogue.
fn page_of(app: &mut App, id: &str) -> Option<String> {
    let cat = probe(app, "settings.rows", json!({ "catalogue": true }));
    cat["rows"]
        .as_array()?
        .iter()
        .find(|r| r["id"].as_str() == Some(id))
        .and_then(|r| r["page"].as_str().map(str::to_string))
}

/// Brings a settings row into view (its page first, then the wheel), as PT's `gotoPref`.
pub(super) fn goto_row(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    let Some(page) = page_of(app, id) else {
        return false;
    };
    if probe(app, "settings.rows", json!({}))["page"].as_str() != Some(page.as_str()) {
        if let Some((x, y, w, h)) = boxed(&probe(app, "settings.rail", json!({ "id": page }))) {
            let _ = app
                .session
                .click((x + w / 2.0) as f32, (y + h / 2.0) as f32);
        }
        let drawn = ctx.until(&format!("the {page} page is drawn"), 6_000, &mut || {
            let r = probe(app, "settings.rows", json!({}));
            r["page"].as_str() == Some(page.as_str()) && r["count"].as_u64().unwrap_or(0) > 0
        });
        if !drawn {
            return false;
        }
    }
    let at = grid_centre(app);
    ctx.until(&format!("row {id} to be in view"), 12_000, &mut || {
        let r = row(app, id);
        if r.is_null() {
            return false;
        }
        if r["visible"] == json!(true) {
            return true;
        }
        let y = r["rect"]["y"].as_f64().unwrap_or(0.0);
        let mid = ui_centre(app, "grid").map_or(300.0, |(_, cy)| f64::from(cy));
        let dy = if y > mid { -160.0 } else { 160.0 };
        let _ = app.session.call(&Request::Pointer {
            x: at.0,
            y: at.1,
            action: PointerAction::Wheel { dx: 0.0, dy },
        });
        false
    })
}

/// Clicks the switch of row `id` (at the right end of its row, inside the 16 px padding).
fn click_switch(app: &mut App, id: &str) -> bool {
    let r = row(app, id);
    let Some((x, y, w, h)) = boxed(&r["rect"]) else {
        return false;
    };
    app.session
        .click((x + w - 16.0 - 18.0) as f32, (y + h / 2.0) as f32)
        .is_ok()
}

/// Flips the switch `id` to `want` and waits for it to read so (PT's `.click()` on the switch).
pub(super) fn set_switch(ctx: &mut Ctx, app: &mut App, id: &str, want: bool) -> bool {
    if !goto_row(ctx, app, id) {
        return false;
    }
    if switch_on(app, id) == Some(want) {
        return true;
    }
    // The pane may still be gliding after the wheel: click once the row holds still.
    let mut last = Value::Null;
    ctx.until(&format!("row {id} to hold still"), 4_000, &mut || {
        let now = row(app, id)["rect"].clone();
        let still = now == last && !now.is_null();
        last = now;
        still
    });
    click_switch(app, id);
    ctx.until(
        &format!("the switch {id} to read {want}"),
        4_000,
        &mut || switch_on(app, id) == Some(want),
    )
}

/// Picks a segment of a pill row by clicking along the row from its right end until the row says
/// `want` (PT's `setPref`). A pill is right-aligned, 16 px in from the row's edge.
pub(super) fn pick_seg(ctx: &mut Ctx, app: &mut App, id: &str, want: &[&str]) -> bool {
    if !goto_row(ctx, app, id) {
        return false;
    }
    if row_is(app, id, want) {
        return true;
    }
    for step in 0..30 {
        let r = row(app, id);
        let Some((x, y, w, h)) = boxed(&r["rect"]) else {
            return false;
        };
        let cx = x + w - 28.0 - f64::from(step) * 10.0;
        if cx < x + w / 2.0 {
            break;
        }
        let _ = app.session.click(cx as f32, (y + h / 2.0) as f32);
        for _ in 0..4 {
            if row_is(app, id, want) {
                return true;
            }
            ctx.settle(25);
        }
    }
    row_is(app, id, want)
}

// ---------------------------------------------------------------- launching

/// Launches the app on `folders` (software renderer: this is a hook and state check, not a pixel one).
fn launch_on(ctx: &mut Ctx, folders: Vec<PathBuf>) -> Result<App, String> {
    ctx.launch(Launch {
        renderer: Some("software"),
        folders,
        tabs_along_the_top: true,
        ..Launch::default()
    })
}

/// The app with the two folders alpha and beta open as two tabs.
fn open_two(ctx: &mut Ctx) -> Result<App, String> {
    let (alpha, beta) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.beta.clone())
    };
    let mut app = launch_on(ctx, vec![alpha, beta])?;
    if !ctx.until("two tabs", 15_000, &mut || tab_count(&mut app) == 2) {
        app.close();
        return Err("the two folders did not open as two tabs".into());
    }
    Ok(app)
}

// ---------------------------------------------------------------- the scenarios

/// PT: run.mjs:999-1081. A run of tool calls, a Stop on a background tab, a question and its answer,
/// the spinner titles; then the log says each move, the folded run and the title meanings, and no
/// line of it carries the marker that was on the screen and in the title.
pub fn agent_diag(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = open_two(ctx)?;
    let result = agent_diag_body(ctx, &mut app);
    app.close();
    result
}

fn agent_diag_body(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    let first = tab_id(app, 0).ok_or("tab 0 has no id")?;
    let second = tab_id(app, 1).ok_or("tab 1 has no id")?;
    wait_for_agent_verdict(app, &first, VERDICT);
    wait_for_agent_verdict(app, &second, VERDICT);
    // A real agent process in tab 0 (review #47): its hooks are a genuine agent's, not a claim.
    stand_in(ctx, app, 0)?;
    blur(app);
    let logs = crate::world::logs_dir(&app.profile);

    // A run of tool calls: three `working` signals, and the screen says the marker.
    let working_hook = say("working");
    at(
        app,
        0,
        &format!("Write-Host '{SECRET} on screen'; {working_hook}; {working_hook}; {working_hook}"),
    );
    let lit = ctx.until("tab 0 working", 8_000, &mut || state(app, 0) == "working");
    ctx.ok(lit, "working hooks light the tab");

    // Finished on a background tab, then looked at.
    at(app, 0, &later(&say("done")));
    tab_click(app, 1);
    let fin = ctx.until("tab 0 finished", 10_000, &mut || {
        state(app, 0) == "finished"
    });
    ctx.ok(fin, "a Stop on a background tab is Finished");
    look(app, 0);
    let cleared = ctx.until("tab 0 cleared", 4_000, &mut || state(app, 0) == "none");
    ctx.ok(cleared, "looking at it takes Finished down");

    // A question on a background tab, answered by work on the tab in front.
    at(app, 0, &later(&say("question")));
    tab_click(app, 1);
    let asked = ctx.until("tab 0 question", 10_000, &mut || {
        state(app, 0) == "question"
    });
    ctx.ok(asked, "a question hook marks it");
    look(app, 0);
    type_line(app, &first, &say("working"));
    let answered = ctx.until("tab 0 working again", 8_000, &mut || {
        state(app, 0) == "working"
    });
    ctx.ok(answered, "work answers it");
    type_line(app, &first, &say("done"));
    let quiet = ctx.until("tab 0 quiet", 8_000, &mut || state(app, 0) == "none");
    ctx.ok(quiet, "and a Stop in front leaves no mark");

    // Titles: idle, two spinner frames, then no agent title at all.
    type_line(
        app,
        &first,
        &format!(
            "{}; Start-Sleep -Milliseconds 400; {}; Start-Sleep -Milliseconds 400; {}; Start-Sleep -Milliseconds 400; $Host.UI.RawUI.WindowTitle = 'pwsh'",
            title_line("2733"),
            title_line("25D0"),
            title_line("25D1")
        ),
    );

    // The page sends every 250 ms and the writer writes every 250 ms.
    let titled = ctx.until("the title lines arrive", 15_000, &mut || {
        log_lines(app)
            .iter()
            .any(|l| field(l, "k") == Some("agent-title") && field(l, "state") == Some("none"))
    });
    ctx.ok(titled, "the title lines arrive");

    let lines = log_lines(app);
    let id = first_hook_id(&lines);
    let hook_seq = hook_sequence(&lines, &id);
    ctx.ok(
        hook_seq == ["working", "workingx2", "done", "question", "working", "done"],
        format!(
            "agent-hook: every signal, the run of three working folded into one line and a count ({})",
            serde_json::to_string(&hook_seq).unwrap_or_default()
        ),
    );

    let mark_seq = mark_sequence(&lines, &id);
    let want = [
        "none>working: hook working",
        "working>done: hook done",
        "done>none: tab looked at",
        "none>question: hook question",
        "question>working: hook working, tab in front",
        "working>none: hook done, tab in front",
    ];
    ctx.ok(
        mark_seq == want,
        format!(
            "agent-mark: each move, with the rule that fired ({})",
            serde_json::to_string(&mark_seq).unwrap_or_default()
        ),
    );
    let marks = marks_of(&lines, &id);
    let named = marks.iter().all(|l| {
        field(l, "agent") == Some("claude") && l.raw.get("held").is_some_and(Value::is_array)
    });
    ctx.ok(named, "each mark names the agent and the marks held");

    let titles = title_sequence(&lines, &id);
    ctx.ok(
        titles == ["idle", "working", "none"],
        format!(
            "agent-title: the meaning only when it changes, not each spinner frame ({})",
            serde_json::to_string(&titles).unwrap_or_default()
        ),
    );

    let leaks = leaking_lines(&log_texts(&logs), SECRET);
    ctx.ok(
        leaks == 0,
        format!("no screen text or title text reaches the log ({leaks} lines carry the marker)"),
    );
    Ok(())
}

/// A real agent process in tab `i` (review #47): its hooks are a genuine agent's, not a claim a look
/// takes back. Leaves the tab inside the stand-in.
fn stand_in(ctx: &mut Ctx, app: &mut App, i: usize) -> Result<(), String> {
    let cmd = agent_proc::command(&app.profile)?;
    at(app, i, &cmd);
    pause(ctx, agent_proc::SETTLE_MS);
    // Rule 1136: a Mac waits for the stand-in's pwsh to run a line before the hook line goes in.
    agent_proc::wait_ready(app);
    Ok(())
}

/// PT: run.mjs:3005-3032, changed by the owner's 2026-10-23 decision (review #45, "Finished at turn end"):
/// a Stop with only "1 shell" in the footer (a run_in_background dev server) is Finished; a workflow's
/// progress row, or Claude's own "still running" summary, keeps the tab Working until it goes.
pub fn background_work(ctx: &mut Ctx) -> Result<(), String> {
    let mut app = open_two(ctx)?;
    let result = background_work_body(ctx, &mut app);
    app.close();
    result
}

/// The footer with a workflow's progress row under the mode line (PT's `7/8 · 11m03s`), as PowerShell.
fn workflow_footer_line() -> String {
    "$r = [string]::new([char]0x2500, 40); Clear-Host; Write-Host $r; Write-Host ([char]0x276F); Write-Host $r; Write-Host '  auto mode on'; Write-Host ('  source-sweep  7/8 ' + [char]0x00B7 + ' 11m03s')"
        .to_string()
}

fn background_work_body(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    for i in 0..2 {
        let id = tab_id(app, i).ok_or("a tab has no id")?;
        wait_for_agent_verdict(app, &id, VERDICT);
    }
    // 1. A dev server's footer count alone: the Stop is the finish. Tab 1 runs a real agent process.
    stand_in(ctx, app, 1)?;
    tab_click(app, 1);
    let id = tab_id(app, 1).ok_or("tab 1 has no id")?;
    type_line(
        app,
        &id,
        &format!(
            "{}; Start-Sleep -Milliseconds 400; {}; {}",
            say("working"),
            footer_line(SHELL_FOOTER),
            say("done")
        ),
    );
    let lit = ctx.until("tab 1 working", 8_000, &mut || state(app, 1) == "working");
    ctx.ok(lit, "a working hook lights the tab");
    tab_click(app, 0);
    let fin = ctx.until("tab 1 finished", 10_000, &mut || {
        state(app, 1) == "finished"
    });
    let s = state(app, 1);
    ctx.ok(
        fin,
        format!("a Stop with only \"1 shell\" in the footer is Finished, not Working ({s})"),
    );
    tab_click(app, 1);
    soft(4_000, &mut || state(app, 1) == "none");

    // 2. A workflow's progress row: the tab stays Working until the row goes.
    type_line(
        app,
        &id,
        &format!(
            "{}; Start-Sleep -Milliseconds 400; {}; {}",
            say("working"),
            workflow_footer_line(),
            say("done")
        ),
    );
    let lit = ctx.until("tab 1 working again", 8_000, &mut || {
        state(app, 1) == "working"
    });
    ctx.ok(lit, "a working hook lights the tab again");
    pause(ctx, 5_500); // past the 1.5 s settle and a 3 s re-read
    let s = state(app, 1);
    ctx.ok(
        s == "working",
        format!("a Stop with a workflow's progress row stays Working ({s})"),
    );
    type_line(app, &id, &footer_line(""));
    let over = ctx.until("tab 1 not working", 8_000, &mut || {
        state(app, 1) != "working"
    });
    let s = state(app, 1);
    ctx.ok(over, format!("the row gone: the work is over ({s})"));
    Ok(())
}

/// PT: run.mjs:3057-3120. The shell's hotkeys are on by default; Ctrl+S toggles the sidebar over a
/// shell; switched off, the shell's Ctrl+S is left to the shell, and Ctrl+Shift+B still works.
pub fn agent_keys(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = {
        let w = ctx.ensure_world();
        w.alpha.clone()
    };
    let mut app = launch_on(ctx, vec![alpha])?;
    let result = agent_keys_body(ctx, &mut app);
    app.close();
    result
}

fn agent_keys_body(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    let one = ctx.until("one tab", 5_000, &mut || tab_count(app) == 1);
    ctx.ok(one, "one tab");

    // Settings opened from the row (PT's gotoPref), then Ctrl+W on its tab: the tabs are the shells.
    let on = open_settings(ctx, app)
        && goto_row(ctx, app, "agent-keys")
        && switch_on(app, "agent-keys") == Some(true);
    ctx.ok(on, "Agent hotkeys is on by default");
    let _ = app.session.key("Ctrl+W");
    soft(4_000, &mut || all_tab_count(app) == 1);
    tab_click(app, 0);
    click_grid(app);

    let _ = app.session.key("Ctrl+T");
    let second = ctx.until("a second tab", 8_000, &mut || tab_count(app) == 2);
    ctx.ok(second, "a second tab");
    click_grid(app);
    let _ = app.session.key("Ctrl+PageUp");
    let before = ctx.until("Ctrl+PageUp to the tab before", 3_000, &mut || {
        front_shell(app) == Some(0)
    });
    ctx.ok(
        before,
        format!(
            "Ctrl+PageUp goes to the tab before ({:?})",
            front_shell(app)
        ),
    );
    let _ = app.session.key("Ctrl+PageDown");
    let after = ctx.until("Ctrl+PageDown to the tab after", 3_000, &mut || {
        front_shell(app) == Some(1)
    });
    ctx.ok(
        after,
        format!("Ctrl+PageDown to the tab after ({:?})", front_shell(app)),
    );

    // The sidebar needs the vertical layout (PT's setPref, then Settings closed with the cog).
    open_settings(ctx, app);
    pick_seg(ctx, app, "tab-layout", &["vertical"]);
    close_settings(app);
    tab_click(app, 0);
    click_grid(app);

    let was = sidebar_mode(app);
    let _ = app.session.key("Ctrl+S");
    let toggled = ctx.until("the sidebar to toggle", 3_000, &mut || {
        sidebar_mode(app) != was
    });
    ctx.ok(
        toggled,
        format!(
            "Ctrl+S over a shell toggles the sidebar ({was} to {})",
            sidebar_mode(app)
        ),
    );
    let _ = app.session.key("Ctrl+S");
    let back = ctx.until("the sidebar to come back", 3_000, &mut || {
        sidebar_mode(app) == was
    });
    ctx.ok(back, format!("and back ({})", sidebar_mode(app)));

    // Off: the shell's key again.
    open_settings(ctx, app);
    let off = set_switch(ctx, app, "agent-keys", false);
    let stored_off = off
        && ctx.until("the switch off is stored", 3_000, &mut || {
            stored(app, "flare.window.agentKeys") == "0"
        });
    ctx.ok(stored_off, "switched off, it is stored");
    close_settings(app);
    tab_click(app, 0);
    click_grid(app);
    let _ = app.session.key("Ctrl+S");
    pause(ctx, 400);
    let kept = sidebar_mode(app) == was;
    ctx.ok(
        kept,
        format!(
            "off, Ctrl+S leaves the sidebar alone ({})",
            sidebar_mode(app)
        ),
    );
    let _ = app.session.key("Ctrl+Shift+B");
    let collapsed = ctx.until("Ctrl+Shift+B to toggle the sidebar", 3_000, &mut || {
        sidebar_mode(app) != was
    });
    ctx.ok(collapsed, "and Ctrl+Shift+B still works");
    Ok(())
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn line(v: Value) -> Line {
        let raw: Map<String, Value> = v.as_object().cloned().unwrap_or_default();
        Line { ms: 0, raw }
    }

    #[test]
    fn a_run_of_one_state_is_one_line_with_its_count() {
        let lines = [
            line(json!({"k": "agent-hook", "id": "t1", "state": "working"})),
            line(json!({"k": "agent-hook", "id": "t1", "state": "working", "repeats": 2})),
            line(json!({"k": "agent-hook", "id": "t1", "state": "done", "repeats": 0})),
            line(json!({"k": "agent-hook", "id": "t2", "state": "question"})),
        ];
        assert_eq!(
            hook_sequence(&lines, "t1"),
            ["working", "workingx2", "done"],
            "a count of 0 is no count, and another tab's hooks are not this tab's"
        );
    }

    #[test]
    fn each_mark_move_reads_from_to_and_why() {
        let lines = [
            line(
                json!({"k": "agent-mark", "id": "t1", "from": "none", "to": "working", "why": "hook working", "held": [], "agent": "claude"}),
            ),
            line(
                json!({"k": "agent-mark", "id": "t2", "from": "none", "to": "done", "why": "hook done"}),
            ),
        ];
        assert_eq!(mark_sequence(&lines, "t1"), ["none>working: hook working"]);
    }

    #[test]
    fn titles_keep_the_meaning_changes_of_their_own_tab() {
        let lines = [
            line(json!({"k": "agent-title", "id": "t1", "state": "idle"})),
            line(json!({"k": "agent-title", "id": "t2", "state": "working"})),
            line(json!({"k": "agent-title", "id": "t1", "state": "working"})),
            line(json!({"k": "agent-title", "id": "t1", "state": "none"})),
        ];
        assert_eq!(title_sequence(&lines, "t1"), ["idle", "working", "none"]);
    }

    #[test]
    fn a_leak_is_each_log_line_holding_the_marker() {
        let texts = vec![
            "{\"k\":\"agent-hook\"}\n{\"k\":\"agent-title\"}".to_string(),
            format!("{{\"k\":\"x\",\"title\":\"{SECRET} fix\"}}\n{{\"note\":\"{SECRET}\"}}"),
        ];
        assert_eq!(leaking_lines(&texts, SECRET), 2);
        assert_eq!(leaking_lines(&texts[..1], SECRET), 0);
    }

    #[test]
    fn the_footer_names_the_shell_only_when_asked() {
        assert!(footer_line(SHELL_FOOTER).contains("1 shell"));
        assert!(footer_line(SHELL_FOOTER).contains("auto mode on"));
        let plain = footer_line("");
        assert!(plain.contains("auto mode on"));
        assert!(!plain.contains("shell"));
    }

    #[test]
    fn a_hook_line_is_written_with_no_raw_escape_bytes() {
        let raw = ps_write(&hook_line("working", &json!({})));
        assert!(raw.starts_with("[Console]::Write([char]27 + '"), "{raw}");
        assert!(raw.ends_with("' + [char]7)"), "{raw}");
        assert!(!raw.contains('\u{1b}') && !raw.contains('\u{7}'));
    }

    #[test]
    fn the_title_line_sets_the_glyph_and_the_marker_text() {
        let t = title_line("2733");
        assert!(t.contains("[char]0x2733"));
        assert!(t.contains(SECRET));
    }
}
