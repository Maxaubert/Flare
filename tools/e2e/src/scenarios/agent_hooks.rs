//! Agent hooks scenarios (Task 26): agentHooks.
//!
//! PT: run.mjs:769-1005 (48 assertions). A shell stands in for Claude: it prints the real
//! `OSC 777;flare-agent` hook lines (`hook_line`) and sets the console title, as Claude does.
//! Two tabs open (alpha, beta); the hooks go to tab 0 while tab 1 is the background or the front.
//! The plugin comes from the app's plugin seam (`startup::production_plugin_dir`): the run asks
//! for it, and the copy lands in the world's LOCALAPPDATA, never the real profile. The six blocks
//! of PT are `hooks_a` to `hooks_f`.
//!
//! PT's `data-attention` line is the tab's mark here (`question`, `finished`, `failed`); its
//! `null` is `none`. PT's two hooks of one failure (the kind's first) are one hook line with a
//! `kind` in Flare (M6 `signal.rs`).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flare_e2e_proto::{PointerAction, Request};
use serde_json::{json, Value};

use crate::ctx::{App, Ctx, Launch};
use crate::standins::agent_proc;
use crate::standins::hooklines::hook_line;
use crate::until::{wait_on, SystemClock};
use crate::world::{prompt_timeout, wait_for_prompt, Shell, CLEAR_HOST, GET_CONTENT};

/// The seam that asks the app for the Claude plugin under `--e2e` (never written as one literal).
const PLUGIN_SEAM: &str = concat!("FLARE_E2E", "_PLUGIN");
/// Where `plugin_dir` looks for the template to install.
const PLUGIN_TEMPLATE_VAR: &str = "FLARE_CLAUDE_PLUGIN_DIR";

/// The idle title Claude sets: the sparkle and its name.
const IDLE: &str = "$Host.UI.RawUI.WindowTitle = [char]0x2733 + ' Claude Code'";

/// The line that counts the plugin folders this shell was handed that really hold a plugin.
const PLUG: &str = "Write-Host ('PLUG-' + @($env:CLAUDE_CODE_PLUGIN_DIRS -split ';' | Where-Object { $_ -and (Test-Path (Join-Path $_ '.claude-plugin\\plugin.json')) }).Count)";

/// A permission box on screen, as Claude draws one.
const BOX: &str = "Write-Host 'Do you want to proceed?'; Write-Host ' 1. Yes'; Write-Host ' 2. No'";

/// A probe's answer, `Null` when it fails (a check on it then reads false).
fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// The `agent.state` answer.
fn agent(app: &mut App) -> Value {
    probe(app, "agent.state", Value::Null)
}

/// How many tabs `agent.state` lists (Settings is not one).
fn tab_count(app: &mut App) -> usize {
    agent(app)["tabs"].as_array().map_or(0, Vec::len)
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

/// PT's `data-attention` line: the mark of a tab that wants a look, else none.
fn line(app: &mut App, i: usize) -> String {
    attention_line(&state(app, i)).to_string()
}

/// The mark word of a state that wants a look, `none` for any other.
fn attention_line(state: &str) -> &str {
    match state {
        "question" | "finished" | "failed" => state,
        _ => "none",
    }
}

/// What the taskbar button was last told: "1 tab needs a look", or empty.
fn badge(app: &mut App) -> String {
    agent(app)["badge"].as_str().unwrap_or("?").to_string()
}

/// The tooltip of tab `i`.
fn tooltip(app: &mut App, i: usize) -> String {
    agent(app)["tabs"][i]["tooltip"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The mark colour of tab `i` (`app.state`, `#rrggbb`).
fn mark_colour(app: &mut App, i: usize) -> String {
    probe(app, "app.state", Value::Null)["tabs"][i]["mark"]["colour"]
        .as_str()
        .unwrap_or("?")
        .to_string()
}

/// The text of tab `id`'s grid.
fn grid_text(app: &mut App, id: &str) -> String {
    crate::world::rows_text(app, id)
}

/// Waits `timeout_ms` for `f` without recording a FAIL (PT's bare `until`, whose result a scenario
/// may ignore).
fn soft(timeout_ms: u64, f: &mut dyn FnMut() -> bool) -> bool {
    let mut clock = SystemClock::new();
    wait_on(&mut clock, timeout_ms, f)
}

/// Holds the scenario for `ms` (PT's `sleep`), through the one wait primitive.
fn pause(ctx: &mut Ctx, ms: u64) {
    let start = Instant::now();
    ctx.until("a pause to pass", ms + 5_000, &mut || {
        start.elapsed() >= Duration::from_millis(ms)
    });
}

/// Samples `f` every poll for `ms` and returns what it flagged (PT's 250 ms sampling loops).
fn sample(ctx: &mut Ctx, ms: u64, f: &mut dyn FnMut() -> Option<String>) -> Vec<String> {
    let start = Instant::now();
    let mut flagged = Vec::new();
    let mut last = Instant::now() - Duration::from_millis(250);
    ctx.until("a sampling window to pass", ms + 5_000, &mut || {
        if last.elapsed() >= Duration::from_millis(250) {
            last = Instant::now();
            if let Some(x) = f() {
                flagged.push(x);
            }
        }
        start.elapsed() >= Duration::from_millis(ms)
    });
    flagged
}

/// Brings tab `i` to the front by clicking its box in the strip.
fn tab_click(app: &mut App, i: usize) -> bool {
    let st = probe(app, "app.state", Value::Null);
    let r = &st["tabs"][i]["rect"];
    let (Some(x), Some(y), Some(w), Some(h)) = (
        r["x"].as_f64(),
        r["y"].as_f64(),
        r["w"].as_f64(),
        r["h"].as_f64(),
    ) else {
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

/// The failure hook with its kind.
fn say_failed(kind: &str) -> String {
    ps_write(&hook_line("failed", &json!({ "kind": kind })))
}

/// Any OSC 777 payload as typed (for payloads that are not ours).
fn osc(payload: &str) -> String {
    ps_write(&format!("\u{1b}]777;{payload}\u{7}"))
}

/// PT `later(cmd)`: waits a moment, so the tab can be left before the hook lands.
fn later(cmd: &str) -> String {
    format!("Start-Sleep -Milliseconds 1500; {cmd}")
}

/// The spinner title at glyph `g` (a code point).
fn spin(g: u32) -> String {
    format!("$Host.UI.RawUI.WindowTitle = [char]0x{g:X} + ' Claude Code'")
}

/// `n` spinner frames about 900 ms apart, alternating the two half circles.
fn frames(n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                "{}; Start-Sleep -Milliseconds 900",
                spin(if i % 2 == 1 { 0x25D1 } else { 0x25D0 })
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
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

/// The centre of the grid (where the settings pane's wheel goes).
fn grid_centre(app: &mut App) -> (f32, f32) {
    let g = probe(app, "ui.rect", json!({ "name": "grid" }));
    let (x, y, w, h) = boxed(&g).unwrap_or((0.0, 36.0, 1000.0, 600.0));
    ((x + w / 2.0) as f32, (y + h / 2.0) as f32)
}

/// Opens Settings from the cog and waits for it to be in front.
fn open_settings(ctx: &mut Ctx, app: &mut App) -> bool {
    let cog = probe(app, "ui.rect", json!({ "name": "cog" }));
    let Some((x, y, w, h)) = boxed(&cog) else {
        return false;
    };
    let _ = app
        .session
        .click((x + w / 2.0) as f32, (y + h / 2.0) as f32);
    ctx.until("Settings is in front", 8_000, &mut || {
        probe(app, "settings.state", Value::Null)["front"] == json!(true)
    })
}

/// The page of a settings row, from the catalogue.
fn page_of(app: &mut App, id: &str) -> Option<String> {
    let cat = probe(app, "settings.rows", json!({ "catalogue": true }));
    cat["rows"]
        .as_array()?
        .iter()
        .find(|r| r["id"].as_str() == Some(id))
        .and_then(|r| r["page"].as_str().map(str::to_string))
}

/// Brings a settings row into view (page first, then the wheel).
fn goto_row(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
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
        let r = probe(app, "settings.row", json!({ "id": id }));
        if r.is_null() {
            return false;
        }
        if r["visible"] == json!(true) {
            return true;
        }
        let y = r["rect"]["y"].as_f64().unwrap_or(0.0);
        let mid = boxed(&probe(app, "ui.rect", json!({ "name": "grid" })))
            .map_or(300.0, |(_, gy, _, gh)| gy + gh / 2.0);
        let dy = if y > mid { -160.0 } else { 160.0 };
        let _ = app.session.call(&Request::Pointer {
            x: at.0,
            y: at.1,
            action: PointerAction::Wheel { dx: 0.0, dy },
        });
        false
    })
}

/// Whether the switch row `id` is on (`None` when the row is not drawn).
fn switch_on(app: &mut App, id: &str) -> Option<bool> {
    probe(app, "settings.row", json!({ "id": id }))["on"].as_bool()
}

/// Clicks the switch of row `id` (at the right end of its row, inside the 16 px padding).
fn click_switch(app: &mut App, id: &str) -> bool {
    let r = probe(app, "settings.row", json!({ "id": id }));
    let Some((x, y, w, h)) = boxed(&r["rect"]) else {
        return false;
    };
    app.session
        .click((x + w - 16.0 - 18.0) as f32, (y + h / 2.0) as f32)
        .is_ok()
}

/// Flips the switch `id` and waits for it to read `want`.
fn set_switch(ctx: &mut Ctx, app: &mut App, id: &str, want: bool) -> bool {
    if !goto_row(ctx, app, id) {
        return false;
    }
    if switch_on(app, id) == Some(want) {
        return true;
    }
    // The pane may still be gliding after the wheel: click once the row holds still.
    let mut last = Value::Null;
    ctx.until(&format!("row {id} to hold still"), 4_000, &mut || {
        let now = probe(app, "settings.row", json!({ "id": id }))["rect"].clone();
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

/// Where the plugin template lives: the repo's own `claude-plugin` folder.
fn plugin_template() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("crates")
        .join("flare-agent")
        .join("claude-plugin")
}

/// A REAL AGENT PROCESS IN TAB `i` (review #47): a copy of PowerShell named `claude.exe` runs inside the
/// tab's shell, so the process poll recognises a Claude there and a hook line printed from inside it is a
/// genuine agent's. Without one, a hook line is a claim about a tab with no agent, and the one look it
/// asks for takes it back. Leaves the tab inside the stand-in.
fn start_stand_in(ctx: &mut Ctx, app: &mut App, profile: &Path, i: usize) -> Result<(), String> {
    let cmd = agent_proc::command(profile)?;
    at(app, i, &cmd);
    pause(ctx, agent_proc::SETTLE_MS);
    at(app, i, IDLE);
    pause(ctx, 2_500);
    Ok(())
}

/// PT: run.mjs:769-1005. Two tabs; the hooks go to the first.
pub fn agent_hooks(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, beta) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.beta.clone())
    };
    let template = plugin_template();
    if !template.join("hooks").join("hooks.json").is_file() {
        return Err(format!(
            "the plugin template is missing: {}",
            template.display()
        ));
    }
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha, beta],
        env: vec![
            (PLUGIN_SEAM.to_string(), "1".to_string()),
            // A plugin folder this machine's own Claude Code environment may hold is not ours to count.
            ("CLAUDE_CODE_PLUGIN_DIRS".to_string(), String::new()),
            (
                PLUGIN_TEMPLATE_VAR.to_string(),
                template.to_string_lossy().into_owned(),
            ),
        ],
        tabs_along_the_top: true,
        ..Launch::default()
    })?;
    let two = ctx.until("two tabs", 15_000, &mut || tab_count(&mut app) == 2);
    if !two {
        app.close();
        return Err("the two folders did not open as two tabs".into());
    }

    let profile = ctx.ensure_world().profile.clone();
    let result = (|| {
        hooks_claim(ctx, &mut app, &profile)?;
        hooks_a(ctx, &mut app, &profile)?;
        hooks_b(ctx, &mut app)?;
        hooks_c(ctx, &mut app)?;
        hooks_d(ctx, &mut app)?;
        hooks_e(ctx, &mut app)?;
        hooks_f(ctx, &mut app)
    })();
    // End idle, so closing asks nothing.
    at(&mut app, 0, "$Host.UI.RawUI.WindowTitle = 'pwsh'");
    app.close();
    result
}

/// PT: run.mjs:802-840. The plugin, a working hook, and a question's life on a background tab.
fn hooks_a(ctx: &mut Ctx, app: &mut App, profile: &Path) -> Result<(), String> {
    // The process poll has said its first word on every shell (a title set before it lands would be
    // cleared by it).
    let first = tab_id(app, 0).ok_or("tab 0 has no id")?;
    crate::world::wait_for_agent_verdict(app, &first, Duration::from_secs(30));

    // THE PLUGIN RIDES THE SHELL'S ENVIRONMENT, and it is real files.
    at(app, 0, PLUG);
    // PT: run.mjs:804
    let plug = ctx.until("PLUG-1", 10_000, &mut || {
        grid_text(app, &first).contains("PLUG-1")
    });
    ctx.ok(plug, "a new shell is handed the Claude Code plugin");
    crate::world::wait_for_agent_verdict(app, &first, Duration::from_secs(30));

    // A genuine agent's hooks: tab 0 runs a real Claude stand-in from here on.
    start_stand_in(ctx, app, profile, 0)?;

    // WORKING, by the hook alone: no title, no output scoring.
    at(app, 0, &say("working"));
    // PT: run.mjs:809
    let working = ctx.until("tab 0 working", 8_000, &mut || state(app, 0) == "working");
    ctx.ok(working, "a working hook lights the tab");

    // A QUESTION on a background tab: the Question line and the badge.
    at(app, 0, &later(&say("question")));
    tab_click(app, 1);
    // PT: run.mjs:813
    let q = ctx.until("tab 0 question", 10_000, &mut || {
        state(app, 0) == "question"
    });
    ctx.ok(q, "a question hook on a background tab marks it");
    // PT: run.mjs:814
    ctx.ok(line(app, 0) == "question", "with the Question line");
    // PT: run.mjs:815
    let counted = ctx.until("the badge to count it", 4_000, &mut || {
        badge(app) == "1 tab needs a look"
    });
    let b = badge(app);
    ctx.ok(counted, format!("and the taskbar badge counts it ({b})"));

    // A QUESTION LASTS UNTIL IT IS ANSWERED (#144): opened and left, it stays.
    look(app, 0);
    pause(ctx, 1000);
    let s = state(app, 0);
    // PT: run.mjs:819
    ctx.ok(
        s == "question" && line(app, 0) == "question",
        format!("opening the tab does not clear the question ({s})"),
    );
    tab_click(app, 1);
    pause(ctx, 500);
    let s = state(app, 0);
    // PT: run.mjs:822
    ctx.ok(
        s == "question",
        format!("leaving it unanswered keeps the line ({s})"),
    );
    let b = badge(app);
    // PT: run.mjs:823
    ctx.ok(
        b == "1 tab needs a look",
        format!("and the badge still counts it ({b})"),
    );

    // Answered by a key no hook reports (a "No", an Esc): Enter with no box on screen.
    look(app, 0);
    let _ = app.session.key("Enter");
    // PT: run.mjs:829
    let answered = ctx.until("the question answered", 4_000, &mut || {
        state(app, 0) == "none"
    });
    ctx.ok(answered, "a key that settles the box answers it");
    // PT: run.mjs:830
    let clear = ctx.until("the badge to let go", 4_000, &mut || badge(app).is_empty());
    ctx.ok(clear, "and the badge lets it go");

    // Asked on the tab in front, it is marked there too; the next hook takes it down. One line
    // typed, so no Enter lands while the question is up.
    at(
        app,
        0,
        &format!(
            "{}; Start-Sleep -Milliseconds 4000; {}",
            later(&say("question")),
            say("working")
        ),
    );
    // PT: run.mjs:836
    let front_q = ctx.until("a question on the front tab", 10_000, &mut || {
        state(app, 0) == "question"
    });
    ctx.ok(front_q, "a question asked on the tab in front marks it");
    let _ = app.session.key("ArrowDown");
    pause(ctx, 1200);
    let s = state(app, 0);
    // PT: run.mjs:839
    ctx.ok(
        s == "question",
        format!("walking the choices is not an answer ({s})"),
    );
    // PT: run.mjs:840
    let w = ctx.until("working after the question", 10_000, &mut || {
        state(app, 0) == "working"
    });
    ctx.ok(w, "a working hook after it answers it");
    // PT: run.mjs:841
    ctx.ok(line(app, 0) == "none", "and no line is left");
    Ok(())
}

/// REVIEW #47, "one check per claim". A capture holding the OSC 777 question signal, cat'ed in a plain
/// shell tab (no agent runs there), claims the tab; the one look it asks for answers "no agent" and the
/// mark clears. No timer or polling loop is behind it. Runs on tab 1, which never holds a stand-in.
fn hooks_claim(ctx: &mut Ctx, app: &mut App, profile: &Path) -> Result<(), String> {
    for i in 0..2 {
        let id = tab_id(app, i).ok_or("a tab has no id")?;
        crate::world::wait_for_agent_verdict(app, &id, Duration::from_secs(30));
    }
    let capture = profile.join("claim-capture.bin");
    fs::write(&capture, b"\x1b]777;flare-agent;state=question\x07").map_err(|e| e.to_string())?;
    at(
        app,
        1,
        &format!(
            "{GET_CONTENT} -Raw -LiteralPath '{}' | Write-Host -NoNewline",
            capture.display()
        ),
    );
    // The claim lands (tab 1 is in front, where a question is raised too) or is already taken back.
    let saw = soft(3_000, &mut || state(app, 1) == "question");
    let gone = ctx.until("the false claim is taken back", 10_000, &mut || {
        state(app, 1) == "none" && agent(app)["tabs"][1]["present"] == json!(false)
    });
    ctx.ok(
        gone,
        format!(
            "a question signal in a plain shell is taken back by the one look it asked for (seen up: {saw})"
        ),
    );
    ctx.ok(
        badge(app).is_empty(),
        "and the taskbar badge counts nothing for it",
    );
    Ok(())
}

/// PT: run.mjs:843-868. A permission prompt answered yes: the Question line, then Working held
/// through a long tool with no hook, then Stop on a background tab.
fn hooks_b(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    at(app, 0, &say("working"));
    at(
        app,
        0,
        &format!(
            "{BOX}; {IDLE}; {}; Start-Sleep -Milliseconds 2500; {CLEAR_HOST}; {}; {}; Start-Sleep -Milliseconds 500; {}",
            say("question"),
            frames(8),
            say("working"),
            say("done")
        ),
    );
    // PT: run.mjs:851
    let q = ctx.until("the permission prompt", 8_000, &mut || {
        state(app, 0) == "question"
    });
    ctx.ok(q, "a permission prompt raises the Question line");
    // PT: run.mjs:852
    let w = ctx.until("working again", 6_000, &mut || state(app, 0) == "working");
    ctx.ok(w, "the spinner after the answer is Working again");
    // Held through the tool: sampled every 250 ms for 5 s, never blank.
    let blank = sample(ctx, 5_000, &mut || {
        let s = state(app, 0);
        (s != "working").then_some(s)
    });
    // PT: run.mjs:861
    ctx.ok(
        blank.is_empty(),
        format!("Working holds through the long tool, with no hook ({blank:?})"),
    );
    tab_click(app, 1);
    // PT: run.mjs:863
    let done = ctx.until("Finished on a background tab", 10_000, &mut || {
        state(app, 0) == "finished"
    });
    ctx.ok(done, "and Stop on a background tab is Finished");
    look(app, 0);
    soft(4_000, &mut || state(app, 0) == "none");
    Ok(())
}

/// PT: run.mjs:870-905. Another agent's work does not take a question down, and a Stop while the
/// spinner carries on is not a finish.
fn hooks_c(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    // ANOTHER AGENT'S WORK DOES NOT TAKE THE QUESTION DOWN (#148): a background subagent's box is
    // up; its sibling's hooks and the main turn's Stop arrive while it waits.
    at(app, 0, &say("working"));
    at(
        app,
        0,
        &format!(
            "{BOX}; {IDLE}; {}; Start-Sleep -Milliseconds 1500; {}; Start-Sleep -Milliseconds 1000; {}; Start-Sleep -Milliseconds 1000; {}; Start-Sleep -Milliseconds 3000; {CLEAR_HOST}; {}; {}",
            say("question"),
            say("working"),
            say("done"),
            say("working"),
            spin(0x25D0),
            say("working")
        ),
    );
    // PT: run.mjs:878
    let q = ctx.until("a subagent's question", 8_000, &mut || {
        state(app, 0) == "question"
    });
    ctx.ok(q, "a subagent asks: the Question line");
    let lost = sample(ctx, 3_500, &mut || {
        let s = state(app, 0);
        (s != "question").then_some(s)
    });
    // PT: run.mjs:885
    ctx.ok(
        lost.is_empty(),
        format!("other agents' signals leave it up while the box shows ({lost:?})"),
    );
    // PT: run.mjs:886
    let w = ctx.until("working once answered", 8_000, &mut || {
        state(app, 0) == "working"
    });
    ctx.ok(w, "answered, the box gone, it is Working");
    at(app, 0, &say("done"));
    at(app, 0, IDLE);

    // NOT FINISHED WHILE BACKGROUND SUBAGENTS STILL WORK (#149): the main turn's Stop comes with the
    // spinner up, and the spinner carries on until the last subagent ends, when the idle star comes.
    look(app, 0);
    soft(4_000, &mut || state(app, 0) == "none");
    at(app, 0, &say("working"));
    soft(8_000, &mut || state(app, 0) == "working");
    at(
        app,
        0,
        &format!(
            "Start-Sleep -Milliseconds 800; {}; {}; Start-Sleep -Milliseconds 300; {}; {IDLE}",
            spin(0x25D0),
            say("done"),
            frames(6)
        ),
    );
    tab_click(app, 1);
    soft(4_000, &mut || state(app, 0) == "working");
    pause(ctx, 1500);
    let early = sample(ctx, 3_000, &mut || {
        let s = state(app, 0);
        (s != "working").then_some(s)
    });
    // PT: run.mjs:906
    ctx.ok(
        early.is_empty(),
        format!("a Stop while the spinner carries on stays Working, not Finished ({early:?})"),
    );
    // PT: run.mjs:907
    let fin = ctx.until("the idle star's finish", 10_000, &mut || {
        state(app, 0) == "finished"
    });
    ctx.ok(
        fin,
        "and the idle star that ends the background work is the finish",
    );
    look(app, 0);
    soft(4_000, &mut || state(app, 0) == "none");
    Ok(())
}

/// PT: run.mjs:912-943. A Stop on a background tab, a finish and a failure on the tab in front
/// while the window was away.
fn hooks_d(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    // DONE on a background tab: the Finished line.
    at(app, 0, &say("working"));
    at(app, 0, &later(&say("done")));
    tab_click(app, 1);
    // PT: run.mjs:916
    let done = ctx.until("a Stop on a background tab", 10_000, &mut || {
        state(app, 0) == "finished"
    });
    ctx.ok(
        done,
        "a Stop hook on a background tab leaves the Finished line",
    );
    look(app, 0);
    soft(4_000, &mut || state(app, 0) == "none");

    // FINISHED ON THE TAB IN FRONT WHILE AWAY, THEN BACK (owner, 2026-10-14, reversed 2026-10-21:
    // "it should keep that completed icon until you click a new tab"): away, the finish is marked
    // and the badge counts it; coming back (focus, the pointer moving) leaves both up, a click on
    // the same tab leaves them, and a click on another tab takes them down (narrowed 2026-10-23).
    for other in [false, true] {
        // Tab 0 in front each round (the last round ended on tab 1).
        look(app, 0);
        at(app, 0, &say("working"));
        soft(8_000, &mut || state(app, 0) == "working");
        let _ = app.session.call(&Request::Focus { on: false });
        at(
            app,
            0,
            &format!("Start-Sleep -Milliseconds 2500; {}", say("done")),
        );
        let away = ctx.until("a finish while away", 10_000, &mut || {
            state(app, 0) == "finished"
        });
        ctx.ok(
            away,
            "a finish on the tab in front, with the window away, is marked",
        );
        let counted = ctx.until("the badge to count it", 4_000, &mut || {
            badge(app) == "1 tab needs a look"
        });
        let b = badge(app);
        ctx.ok(counted, format!("and the badge counts it ({b})"));
        for (x, y) in [
            (500.0, 300.0),
            (507.0, 307.0),
            (513.0, 313.0),
            (520.0, 320.0),
        ] {
            let _ = app.session.call(&Request::Pointer {
                x,
                y,
                action: PointerAction::Move,
            });
        }
        let _ = app.session.call(&Request::Focus { on: true });
        // PT waits 1200 ms: a late clear (a timer, a title recheck) must not slip past.
        let left = ctx.poll(1_200, &mut || {
            state(app, 0) != "finished" || badge(app) != "1 tab needs a look"
        });
        // PT: run.mjs:924
        ctx.ok(
            !left && state(app, 0) == "finished",
            "back, the pointer moving and the focus returning leave the mark up",
        );
        // PT: run.mjs:925
        let b = badge(app);
        ctx.ok(
            b == "1 tab needs a look",
            format!("and the badge with it ({b})"),
        );
        if !other {
            // A click on the tab already in front changes nothing: the mark stays up (owner, 2026-10-23).
            tab_click(app, 0);
            let left = ctx.poll(1_200, &mut || {
                state(app, 0) != "finished" || badge(app) != "1 tab needs a look"
            });
            ctx.ok(
                !left && state(app, 0) == "finished",
                "a click on the tab already in front leaves the mark and the badge up",
            );
        }
        // Another tab in front takes it down, off the tab left too.
        tab_click(app, 1);
        let down = ctx.until(
            "a click on another tab to take the mark down",
            4_000,
            &mut || state(app, 0) == "none",
        );
        ctx.ok(
            down,
            "a click on another tab takes it down, off the tab left too",
        );
        let clear = ctx.until("the badge to go", 4_000, &mut || badge(app).is_empty());
        let b = badge(app);
        ctx.ok(clear, format!("and the badge with it ({b})"));
    }

    // A FAILURE ON THE TAB IN FRONT WHILE AWAY IS HELD TOO. Tab 0 is brought back to the front
    // first (the loop ended on tab 1), so the failure is raised on the tab in front.
    look(app, 0);
    at(app, 0, &say("working"));
    soft(8_000, &mut || state(app, 0) == "working");
    let _ = app.session.call(&Request::Focus { on: false });
    at(
        app,
        0,
        &format!("Start-Sleep -Milliseconds 2500; {}", say("failed")),
    );
    // PT: run.mjs:936
    let away = ctx.until("a failure while away", 10_000, &mut || {
        state(app, 0) == "failed"
    });
    ctx.ok(
        away,
        "a failure on the tab in front, with the window away, is marked",
    );
    for (x, y) in [(500.0, 300.0), (520.0, 320.0)] {
        let _ = app.session.call(&Request::Pointer {
            x,
            y,
            action: PointerAction::Move,
        });
    }
    let _ = app.session.call(&Request::Focus { on: true });
    let left = ctx.poll(1_200, &mut || state(app, 0) != "failed");
    // PT: run.mjs:941
    ctx.ok(!left, "back, the Failed mark stays up");
    tab_click(app, 1);
    let down = ctx.until(
        "another tab's click to take the Failed mark down",
        4_000,
        &mut || state(app, 0) == "none",
    );
    // PT: run.mjs:943
    ctx.ok(
        down,
        "and a click on another tab takes the Failed mark down",
    );
    Ok(())
}

/// PT: run.mjs:945-969. A failure with its kind, the badge, and the Failed switch.
fn hooks_e(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    // FAILED, with its kind.
    at(app, 0, &say("working"));
    at(app, 0, &later(&say_failed("rate_limit")));
    tab_click(app, 1);
    // PT: run.mjs:949
    let failed = ctx.until("a StopFailure", 10_000, &mut || state(app, 0) == "failed");
    ctx.ok(
        failed,
        "a StopFailure hook on a background tab marks it Failed",
    );
    // PT: run.mjs:950
    ctx.ok(line(app, 0) == "failed", "with the Failed line");
    let tip = tooltip(app, 0);
    // PT: run.mjs:952
    ctx.ok(
        tip.ends_with("\nFailed: rate limit"),
        format!("and the tooltip says what failed ({tip:?})"),
    );
    // PT: run.mjs:953
    let counted = ctx.until("the badge to count it", 4_000, &mut || {
        badge(app) == "1 tab needs a look"
    });
    ctx.ok(counted, "the badge counts a failed tab");
    let failed_colour = mark_colour(app, 0);
    ctx.shot(app, "agent-hooks-failed");

    // THE FAILED SWITCH: off, the line goes and the turn reads as finished.
    open_settings(ctx, app);
    let reached = goto_row(ctx, app, "agent-failed-on");
    // PT: run.mjs:962
    ctx.ok(
        reached && switch_on(app, "agent-failed-on") == Some(true),
        "the Failed indicator is on by default",
    );
    click_switch(app, "agent-failed-on");
    // PT: run.mjs:964
    let finished = ctx.until("the Finished line instead", 4_000, &mut || {
        line(app, 0) == "finished"
    });
    ctx.ok(
        finished,
        "switched off, no Failed line: the Finished one shows instead",
    );
    let done_colour = mark_colour(app, 0);
    // PT: run.mjs:966
    ctx.ok(
        failed_colour != done_colour,
        format!("the Failed line is a colour of its own ({failed_colour} vs {done_colour})"),
    );
    click_switch(app, "agent-failed-on");
    // PT: run.mjs:968
    let again = ctx.until("Failed again", 4_000, &mut || line(app, 0) == "failed");
    ctx.ok(again, "and back on, it is Failed again");
    let hooks_row = goto_row(ctx, app, "agent-hooks");
    // PT: run.mjs:969
    ctx.ok(
        hooks_row && switch_on(app, "agent-hooks") == Some(true),
        "Exact status from Claude Code is on by default",
    );
    Ok(())
}

/// PT: run.mjs:971-1005. An Esc is not a finish; other payloads change nothing; a hooked session's
/// screen is not read; with the hooks off a new shell has no plugin.
fn hooks_f(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    // AN ESC: working, then the idle title with no Stop. Not a finish.
    look(app, 0);
    soft(4_000, &mut || state(app, 0) == "none");
    at(app, 0, &say("working"));
    soft(8_000, &mut || state(app, 0) == "working");
    at(app, 0, &later(IDLE));
    tab_click(app, 1);
    // PT: run.mjs:978
    let stopped = ctx.until("the work to stop", 10_000, &mut || {
        state(app, 0) != "working"
    });
    ctx.ok(stopped, "an idle title after a working hook stops the work");
    pause(ctx, 1000);
    let s = state(app, 0);
    // PT: run.mjs:980
    ctx.ok(
        s == "none" && line(app, 0) == "none",
        format!("and leaves no Finished line, an Esc is not a finish ({s})"),
    );
    // PT: run.mjs:981
    ctx.ok(badge(app).is_empty(), "nor anything for the badge");

    // ANOTHER OSC 777, and our prefix with a state we do not know: nothing.
    at(
        app,
        0,
        &later(&format!(
            "{}; {}; {}",
            osc("notify;Build;done"),
            osc("flare-agent;state=sleeping"),
            osc("flare-agentx;state=failed")
        )),
    );
    tab_click(app, 1);
    pause(ctx, 2500);
    let s = state(app, 0);
    // PT: run.mjs:987
    ctx.ok(
        s == "none",
        format!("another OSC 777 payload changes nothing ({s})"),
    );

    // A HOOKED SESSION'S SCREEN IS NOT READ FOR A QUESTION.
    at(
        app,
        0,
        &later(&format!(
            "Write-Host 'Enter to select, Esc to cancel'; {IDLE}"
        )),
    );
    tab_click(app, 1);
    pause(ctx, 2500);
    let s = state(app, 0);
    // PT: run.mjs:994
    ctx.ok(
        s == "none",
        format!("the question footer on screen does not mark a hooked session ({s})"),
    );

    // OFF MEANS OFF: a shell opened with the setting off gets no plugin.
    let before: Vec<String> = (0..tab_count(app)).filter_map(|i| tab_id(app, i)).collect();
    // Settings is behind the shell tab now: the cog brings it back to the front.
    open_settings(ctx, app);
    set_switch(ctx, app, "agent-hooks", false);
    tab_click(app, 1);
    let _ = app.session.key("Ctrl+T");
    let opened = ctx.until("a new tab", 8_000, &mut || {
        tab_count(app) == before.len() + 1
    });
    let fresh = (0..tab_count(app))
        .filter_map(|i| tab_id(app, i))
        .find(|id| !before.contains(id));
    let mut none = false;
    if let (true, Some(id)) = (opened, fresh) {
        type_line(app, &id, PLUG);
        none = ctx.until("PLUG-0", 10_000, &mut || {
            grid_text(app, &id).contains("PLUG-0")
        });
    }
    // PT: run.mjs:1004
    ctx.ok(none, "with the setting off, a new shell has no plugin");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hook_lines_are_typed_as_escapes_not_raw_bytes() {
        let s = say("working");
        assert!(s.starts_with("[Console]::Write([char]27 + ']777;flare-agent;state=working'"));
        assert!(s.ends_with("+ [char]7)"));
        assert!(!s.contains('\u{1b}') && !s.contains('\u{7}'));
    }

    #[test]
    fn a_failure_carries_its_kind() {
        assert!(say_failed("rate_limit").contains("state=failed;kind=rate_limit"));
    }

    #[test]
    fn a_foreign_payload_is_written_as_given() {
        let s = osc("notify;Build;done");
        assert!(s.contains("']777;notify;Build;done'"));
        let ours = osc("flare-agentx;state=failed");
        assert!(ours.contains("flare-agentx;state=failed"));
    }

    #[test]
    fn the_spinner_frames_alternate_the_two_half_circles() {
        let f = frames(3);
        assert_eq!(f.matches("Start-Sleep -Milliseconds 900").count(), 3);
        let first = f.find("0x25D0").expect("first frame");
        let second = f.find("0x25D1").expect("second frame");
        assert!(first < second);
    }

    #[test]
    fn the_plugin_template_is_the_repos_own_and_holds_its_hooks() {
        let t = plugin_template();
        assert!(t.join("hooks").join("hooks.json").is_file(), "{t:?}");
        assert!(t.join(".claude-plugin").join("plugin.json").is_file());
    }

    #[test]
    fn the_seam_name_is_the_one_the_app_reads() {
        assert_eq!(PLUGIN_SEAM, "FLARE_E2E_PLUGIN");
    }

    #[test]
    fn a_line_is_a_mark_only_for_the_three_that_want_a_look() {
        for (s, want) in [
            ("question", "question"),
            ("finished", "finished"),
            ("failed", "failed"),
            ("working", "none"),
            ("none", "none"),
        ] {
            assert_eq!(attention_line(s), want);
        }
    }
}
