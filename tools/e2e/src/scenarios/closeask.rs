//! The close question scenarios (Task 24): closeAsk.
//!
//! PT: run.mjs:3482-3530. PT's shell pretends to be Claude by typing its window title; Flare runs a
//! real `claude` stand-in (FakeClaude) from the PATH the launch sets. It sets its idle title (the
//! sparkle), waits for the scenario's go file, then sets the working title and its hook line. Closes
//! go through Ctrl+W on the tab in front, which is the same request the tab's close button makes
//! (`request_close`); the hover close button has no probe rectangle yet.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ctx::{App, Ctx, Launch};
use crate::standins::agent_cmd::{FakeClaude, Step};
use crate::standins::hooklines::hook_line;
use crate::world::{Shell, prompt_timeout, wait_for_prompt};

/// The idle title Claude sets (U+2733, the sparkle) and the working one (U+25D0, the half circle).
const IDLE_TITLE: &str = "\u{2733} Claude Code";
const WORKING_TITLE: &str = "\u{25D0} Claude Code";

/// The file whose appearance tells the stand-in to start working.
const GO_FILE: &str = "go-working";

/// The probe's answer for `name`, `Null` when it fails (a check on it then reads false).
fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// The `app.state` answer (tabs with their kind, mark and rect, and the dialog).
fn app_state(app: &mut App) -> Value {
    probe(app, "app.state", Value::Null)
}

/// How many tabs the app shows now.
fn tab_count(app: &mut App) -> usize {
    app_state(app)["tabs"].as_array().map_or(0, Vec::len)
}

/// The id of the tab at `index` (strip order), if there is one.
fn tab_id(app: &mut App, index: usize) -> Option<String> {
    app_state(app)["tabs"][index]["id"]
        .as_str()
        .map(str::to_string)
}

/// True when the tab at `index` hosts Claude and that Claude is not working (idle).
fn agent_idle(app: &mut App, index: usize) -> bool {
    let st = app_state(app);
    let tab = &st["tabs"][index];
    tab["kind"] == json!("claude") && tab["mark"]["state"] != json!("working")
}

/// True when the tab at `index` shows the working mark.
fn agent_working(app: &mut App, index: usize) -> bool {
    app_state(app)["tabs"][index]["mark"]["state"] == json!("working")
}

/// True while the close question is up.
fn dialog_open(app: &mut App) -> bool {
    app_state(app)["dialog"]["open"] == json!(true)
}

/// The question's text (its title and body), as the app reports them.
fn dialog_text(app: &mut App) -> String {
    let d = &app_state(app)["dialog"];
    format!(
        "{} {}",
        d["title"].as_str().unwrap_or(""),
        d["body"].as_str().unwrap_or("")
    )
}

/// The `dialog.focus` answer: "primary", "secondary" or "none".
fn dialog_focus(app: &mut App) -> String {
    probe(app, "dialog.focus", Value::Null)["focused"]
        .as_str()
        .unwrap_or("none")
        .to_string()
}

/// Makes the tab at `index` the front one by clicking its middle in the strip.
fn bring_to_front(app: &mut App, index: usize) -> bool {
    let st = app_state(app);
    let r = &st["tabs"][index]["rect"];
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

/// The steps of the stand-in: the idle title at once, then, once `go` exists, the working title
/// and its hook line. It stays open for an hour, as a running agent does.
fn steps_script(go: &Path) -> String {
    // The hook line `hook_line` writes, minus its ESC and BEL, which the script adds itself.
    let hook = hook_line("working", &json!({}));
    let hook = hook.trim_start_matches('\u{1b}').trim_end_matches('\u{7}');
    format!(
        "$ErrorActionPreference = 'Stop'\r\n\
         $e = [string][char]27\r\n\
         $b = [string][char]7\r\n\
         [Console]::Write($e + ']0;{IDLE_TITLE}' + $b)\r\n\
         while (-not (Test-Path -LiteralPath '{go}')) {{ Start-Sleep -Milliseconds 50 }}\r\n\
         [Console]::Write($e + ']0;{WORKING_TITLE}' + $b)\r\n\
         [Console]::Write($e + '{hook}' + $b)\r\n\
         Start-Sleep -Seconds 3600\r\n",
        go = go.display(),
    )
}

/// Writes the stand-in's steps: the title script of `FakeClaude` is replaced by [`steps_script`]
/// (with a byte-order mark, as the stand-in writes it, so Windows PowerShell reads the glyphs).
fn write_steps(claude: &mut FakeClaude, go: &Path) -> Result<(), String> {
    claude.script(&[Step::Title(IDLE_TITLE.into())]);
    let body = steps_script(go);
    let bytes = [b"\xEF\xBB\xBF".as_slice(), body.as_bytes()].concat();
    std::fs::write(claude.bin().join("claude-steps.ps1"), bytes)
        .map_err(|e| format!("write the stand-in's steps: {e}"))
}

/// PT: run.mjs:3482-3530. Two tabs; the second hosts an idle, then a working, Claude.
pub fn close_ask(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, beta, mut claude) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.beta.clone(), FakeClaude::new(w))
    };
    let go: PathBuf = claude.dir.join(GO_FILE);
    write_steps(&mut claude, &go)?;

    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha, beta],
        env: claude.env(),
        ..Launch::default()
    })?;
    let two = ctx.until("two tabs", 15_000, &mut || tab_count(&mut app) == 2);
    if !two {
        app.close();
        return Err("the two folders did not open as two tabs".into());
    }

    // The second tab hosts the agent: its shell takes `claude`, which the PATH resolves to the
    // stand-in. (PT waits for the process poll's first verdict here; Flare's poll is the hub's, and
    // the presence check below waits for it.)
    let agent_tab = tab_id(&mut app, 1).ok_or("the second tab has no id")?;
    bring_to_front(&mut app, 1);
    wait_for_prompt(
        &mut app,
        &agent_tab,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    );
    app.session.type_text("claude")?;
    app.session.key("Enter")?;

    // PT: run.mjs:3493-3494. An agent is present and idle in the tab.
    let idle = ctx.until("an idle agent in the second tab", 8000, &mut || {
        agent_idle(&mut app, 1)
    });
    ctx.ok(idle, "an agent is present and IDLE in the tab");

    // PT: run.mjs:3495-3497. Closing its tab asks, though the agent is only idle.
    app.session.key("Ctrl+W")?;
    let asked = ctx.until("the close question for an idle agent", 4000, &mut || {
        dialog_open(&mut app)
    });
    ctx.ok(asked, "closing its tab asks, though the agent is only idle");
    ctx.ok(
        dialog_text(&mut app).contains("Claude"),
        "and the question names the agent",
    );

    // PT: run.mjs:3500-3506. Tab and Shift+Tab stay inside the question.
    for _ in 0..5 {
        app.session.key("Tab")?;
    }
    ctx.ok(
        dialog_focus(&mut app) != "none",
        "Tab stays inside the question",
    );
    app.session.key("Shift+Tab")?;
    ctx.ok(dialog_focus(&mut app) != "none", "and so does Shift+Tab");

    // PT: run.mjs:3507-3511. A press over the title bar is a press outside: it cancels. The point
    // is 260 css px in from the window's right edge, on the bar's middle line (the bar is 36 px).
    let st = app_state(&mut app);
    let scale = st["scale"].as_f64().unwrap_or(1.0).max(0.1);
    let client_w = st["window"]["client_w"].as_f64().unwrap_or(0.0) / scale;
    app.session.click((client_w - 260.0) as f32, 18.0)?;
    let cancelled = ctx.until("the question to cancel", 3000, &mut || {
        !dialog_open(&mut app)
    });
    ctx.ok(cancelled, "a press over the title bar cancels it");

    // PT: run.mjs:3512. Cancel keeps the tab.
    ctx.ok(tab_count(&mut app) == 2, "Cancel keeps the tab");

    // PT: run.mjs:3513-3515. Escape cancels too.
    app.session.key("Ctrl+W")?;
    ctx.until("the question again", 4000, &mut || dialog_open(&mut app));
    app.session.key("Escape")?;
    let escaped = ctx.until("the question to close on Escape", 3000, &mut || {
        !dialog_open(&mut app)
    });
    ctx.ok(escaped, "and Escape cancels too");

    // PT: run.mjs:3516-3519. Mid-answer, the agent works: the stand-in's go file.
    std::fs::write(&go, b"").map_err(|e| format!("the go file: {e}"))?;
    ctx.until("the agent to start working", 8000, &mut || {
        agent_working(&mut app, 1)
    });

    // PT: run.mjs:3523-3526. A working tab's close asks too. (PT hovers the tab first: the hover
    // close button needs room; Ctrl+W on the front tab asks the same question.)
    app.session.key("Ctrl+W")?;
    let working_asked = ctx.until("the close question for a working agent", 4000, &mut || {
        dialog_open(&mut app)
    });
    ctx.ok(working_asked, "a working agent asks too");
    ctx.ok(
        dialog_text(&mut app).contains("working for"),
        "and says how long it has been working",
    );

    // PT: run.mjs:3527-3528. Confirming closes the tab. The primary button holds the keys on open
    // (the probe `dialog.focus` says so), and Enter presses it.
    app.session.key("Enter")?;
    let closed = ctx.until("the agent tab to close", 4000, &mut || {
        tab_count(&mut app) == 1
    });
    ctx.ok(closed, "confirming closes the tab");

    // PT: run.mjs:3529-3530. A plain shell has nothing to lose: no question.
    bring_to_front(&mut app, 0);
    app.session.key("Ctrl+W")?;
    let empty = ctx.until(
        "the start page after the plain tab closes",
        4000,
        &mut || probe(&mut app, "start.state", Value::Null)["shown"] == json!(true),
    );
    ctx.ok(empty, "a tab with no agent closes unasked");

    app.close();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stand_in_is_idle_until_the_go_file_then_works_with_its_hook() {
        let s = steps_script(Path::new(r"C:\tmp\flare-e2e\go-working"));
        let idle = s.find(IDLE_TITLE).expect("the idle title is written");
        let wait = s
            .find("Test-Path")
            .expect("the script waits for the go file");
        let working = s.find(WORKING_TITLE).expect("the working title is written");
        assert!(
            idle < wait && wait < working,
            "idle, then the wait, then working"
        );
        assert!(s.contains(r"C:\tmp\flare-e2e\go-working"));
        assert!(
            s.contains("state=working"),
            "the working hook is the agent's own word"
        );
        assert!(
            !s.contains('\u{1b}'),
            "the ESC and BEL are added by the script, not embedded"
        );
    }
}
