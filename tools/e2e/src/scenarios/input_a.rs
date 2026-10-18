//! Input A scenarios (Task 29): caretHold, mouseReport, clickCaret, findKey, scrollbar, paste.
//!
//! PT: run.mjs:1200-1766. One `ctx.ok` per PT `ok(`, in PT's order, with a `// PT: run.mjs:<line>`
//! above each. The probes they read: `grid.text`, `app.state`, `ui.rect`, `dpi.state`, `clip.last`
//! (core), and `term.mouse`, `scrollbar.rect` (`e2e_probes/input_a.rs`). `ime.rect` and the pointer
//! shape are not exported yet (see the checks that name them).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flare_e2e_proto::{PointerAction, Reply, Request};
use serde_json::{Value, json};

use crate::ctx::{App, Ctx, Launch};
use crate::until::{SystemClock, wait_on};
use crate::world::{Shell, mute_psreadline_paste, prompt_timeout, rows_text, wait_for_prompt};

/// The stand-in that asks for the mouse (PT mouseReport's probe, in PowerShell). It turns on VT
/// input so a mouse report reaches its key reads as the `ESC [ <` sequence, as Claude's own runtime
/// gets it; then it prints `REPORT <char code>` per key until `q`.
const MOUSE_PROBE: &str = r##"$e = [char]27
$log = Join-Path $PSScriptRoot 'agent.log'
Add-Content $log 'start'
Add-Type -Namespace Flare -Name Vt -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("kernel32.dll")] public static extern IntPtr GetStdHandle(int n);
[System.Runtime.InteropServices.DllImport("kernel32.dll")] public static extern bool GetConsoleMode(IntPtr h, ref int m);
[System.Runtime.InteropServices.DllImport("kernel32.dll")] public static extern bool SetConsoleMode(IntPtr h, int m);
'@
$h = [Flare.Vt]::GetStdHandle(-10)
$m = 0
[void][Flare.Vt]::GetConsoleMode($h, [ref]$m)
[void][Flare.Vt]::SetConsoleMode($h, 0x0200 -bor 0x0010 -bor 0x0080)
[Console]::Write($e + '[?1000h' + $e + '[?1002h' + $e + '[?1006h' + 'PROBE-READY' + "`r`n")
while ($true) {
  $k = [Console]::ReadKey($true)
  if ($k.KeyChar -eq 'q') { break }
  [Console]::Write('REPORT ' + [int]$k.KeyChar + "`r`n")
}
[Console]::Write($e + '[?1006l' + $e + '[?1002l' + $e + '[?1000l')
"##;

/// The scale the pixel and geometry checks run at (225 percent, `WM_DPICHANGED` 216).
const SCALE_225: u32 = 216;

/// The stand-in agent (PT caretHold's `inline-agent.cjs`): nine blank rows and an input line. Each
/// 150 ms frame parks the cursor eight rows up and writes `stream N` there; a key brings the cursor
/// back to the input line and redraws it. `q` ends it.
const INLINE_AGENT: &str = r#"$e = [char]27
$log = Join-Path $PSScriptRoot 'agent.log'
Add-Content $log 'start'
$text = ''
$parked = $false
$n = 0
[Console]::Write(("`r`n" * 9) + 'INPUT> ')
$sw = [Diagnostics.Stopwatch]::StartNew()
$last = -1000
while ($true) {
  if ($sw.ElapsedMilliseconds - $last -ge 150) {
    $last = $sw.ElapsedMilliseconds
    $n++
    if ($n -le 40 -or $n % 20 -eq 0) { Add-Content $log ("frame " + $n) }
    if (-not $parked) { [Console]::Write("$e[8A"); $parked = $true }
    [Console]::Write("`rstream $n$e[K")
  }
  if ([Console]::KeyAvailable) {
    $k = [Console]::ReadKey($true)
    Add-Content $log ("key " + [int]$k.KeyChar)
    if ($k.KeyChar -eq 'q') { if ($parked) { [Console]::Write("$e[8B") }; [Console]::Write("`rINPUT> $text$e[K`r`n"); break }
    $text += $k.KeyChar
    if ($parked) { [Console]::Write("$e[8B"); $parked = $false }
    [Console]::Write("`rINPUT> $text$e[K")
  } else {
    Start-Sleep -Milliseconds 15
  }
}
"#;

/// The one wait of this group (PT's bare `until`): true when `f` held within `timeout_ms`. Unlike
/// `Ctx::until` it records no FAIL of its own, so the check that reads its answer is the one
/// `ctx.ok` for it, and the count stays PT's count of `ok(` calls. `_what` names the wait for a
/// reader of the code.
fn wait_for(_what: &str, timeout_ms: u64, f: &mut dyn FnMut() -> bool) -> bool {
    let mut clock = SystemClock::new();
    wait_on(&mut clock, timeout_ms, f)
}

/// Holds the scenario for `ms` (PT's `sleep`).
fn pause(ms: u64) {
    let start = Instant::now();
    wait_for("a pause to pass", ms + 5_000, &mut || {
        start.elapsed() >= Duration::from_millis(ms)
    });
}

/// A probe's answer, `Null` when it fails (a check on it then reads false).
fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// The id of the tab in front (`app.state`'s `front` flag), empty when none is.
fn front_id(app: &mut App) -> String {
    probe(app, "app.state", Value::Null)["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|t| t["front"] == json!(true)))
        .and_then(|t| t["id"].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The front tab's grid text, one row per line.
fn text(app: &mut App) -> String {
    let id = front_id(app);
    rows_text(app, &id)
}

/// The front tab's `grid.text` answer: rows, cursor, columns.
fn grid(app: &mut App) -> Value {
    let id = front_id(app);
    probe(app, "grid.text", json!({ "tab": id }))
}

/// A `ui.rect` answer in css px: x, y, w, h.
fn css_rect(v: &Value) -> Option<(f64, f64, f64, f64)> {
    Some((
        v["x"].as_f64()?,
        v["y"].as_f64()?,
        v["w"].as_f64()?,
        v["h"].as_f64()?,
    ))
}

/// A `ui.rect` answer in device px: dx, dy, dw, dh.
fn dev_rect(v: &Value) -> Option<(i64, i64, i64, i64)> {
    Some((
        v["dx"].as_i64()?,
        v["dy"].as_i64()?,
        v["dw"].as_i64()?,
        v["dh"].as_i64()?,
    ))
}

/// The centre of the grid in css px: where a click lands on the terminal.
fn grid_centre(app: &mut App) -> (f32, f32) {
    let v = probe(app, "ui.rect", json!({ "name": "grid" }));
    css_rect(&v).map_or((0.0, 0.0), |(x, y, w, h)| {
        ((x + w / 2.0) as f32, (y + h / 2.0) as f32)
    })
}

/// The last row whose text holds `needle`, as a row index of the grid.
fn row_of(grid: &Value, needle: &str) -> Option<usize> {
    grid["rows"]
        .as_array()?
        .iter()
        .enumerate()
        .rev()
        .find(|(_, r)| r.as_str().unwrap_or("").contains(needle))
        .map(|(i, _)| i)
}

/// The last row with a PowerShell prompt that also holds `word` (PT's `PS [^>]*> ?word`).
fn prompt_row_with(grid: &Value, word: &str) -> Option<usize> {
    grid["rows"]
        .as_array()?
        .iter()
        .enumerate()
        .rev()
        .find(|(_, r)| {
            let t = r.as_str().unwrap_or("");
            t.contains("PS ") && t.contains(word)
        })
        .map(|(i, _)| i)
}

/// The last three non-empty rows of the front grid, for a reason a reader can act on.
fn last_rows(app: &mut App) -> String {
    let t = text(app);
    let rows: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty()).collect();
    let n = rows.len();
    rows[n.saturating_sub(3)..].join(" / ")
}

/// The last six lines of the stand-in's own log beside it (what it saw and drew), for a reason.
fn log_tail(agent: &Path) -> String {
    let text = std::fs::read_to_string(agent.with_file_name("agent.log")).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();
    lines[n.saturating_sub(6)..].join(" / ")
}

/// Waits for the front shell's prompt. True when it showed one.
fn wait_prompt(app: &mut App) -> bool {
    let id = front_id(app);
    wait_for_prompt(
        app,
        &id,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    )
}

/// Types `line` into the front shell and presses Enter.
fn run_line(app: &mut App, line: &str) -> Result<(), String> {
    app.session.type_text(line)?;
    app.session.key("Enter")
}

/// Whether the find bar is open (`app.state`'s `find.open`).
fn find_open(app: &mut App) -> bool {
    probe(app, "app.state", Value::Null)["find"]["open"] == json!(true)
}

/// Whether the front shell has asked for the mouse (`term.mouse`'s `on`).
fn mouse_on(app: &mut App) -> bool {
    probe(app, "term.mouse", Value::Null)["on"] == json!(true)
}

/// One pixel of the last snapshot, as sRGB; None when the take or the read failed.
fn pixel(app: &mut App, x: i64, y: i64) -> Option<[u8; 3]> {
    match app.session.call(&Request::Pixel {
        x: x.max(0) as u32,
        y: y.max(0) as u32,
    }) {
        Ok(Reply::Pixel { rgba }) => Some([rgba[0], rgba[1], rgba[2]]),
        _ => None,
    }
}

/// Puts `text` on the in-memory clipboard. Empty string is the app's answer when it refused.
fn put_text(app: &mut App, text: &str) -> Result<(), String> {
    match app.session.call(&Request::ClipSet {
        text: Some(text.to_string()),
        png_b64: None,
    })? {
        Reply::Ok => Ok(()),
        Reply::Err { message } => Err(message),
        other => Err(format!("the clipboard answered {other:?}")),
    }
}

/// Counts the copies of `word` on the front grid.
fn count(app: &mut App, word: &str) -> usize {
    text(app).matches(word).count()
}

/// Escape clears the shell's line, so each paste is counted on its own (PT `clear`).
fn clear_line(app: &mut App) -> Result<(), String> {
    app.session.key("Escape")?;
    pause(300);
    Ok(())
}

/// The grid position of the character at `offset` into the last row holding `needle`, in css px.
/// The point is 20 percent into the cell, so a click there puts the caret before that character.
fn caret_point(app: &mut App, needle: &str, offset: usize) -> Option<(f32, f32)> {
    let g = grid(app);
    let row = row_of(&g, needle)?;
    let line = g["rows"][row].as_str()?;
    let col = line.rfind(needle)? + offset;
    let d = probe(app, "dpi.state", Value::Null);
    let pad = d["padding"].as_f64()?;
    let cw = d["cell_w"].as_f64()?;
    let ch = d["cell_h"].as_f64()?;
    let k = d["scale"].as_f64()?;
    let (gx, gy, _, _) = css_rect(&probe(app, "ui.rect", json!({ "name": "grid" })))?;
    let x = gx + (pad + col as f64 * cw + 0.2 * cw) / k;
    let y = gy + (pad + row as f64 * ch + ch / 2.0) / k;
    Some((x as f32, y as f32))
}

/// Whether the IME rectangle the window reports sits on grid row `row`, within 2 device px (PT:
/// the textarea on the input line). `ime.rect` is not exported by the app yet: the IME area is set
/// on the winit window in `window.rs` and nothing keeps it for a probe. Without the probe the
/// answer is no, and the detail says why.
fn ime_on_row(app: &mut App, row: Option<usize>) -> (bool, String) {
    let ime = match app.session.probe("ime.rect", Value::Null) {
        Ok(v) => v,
        Err(e) => return (false, format!("ime.rect is not exported ({e})")),
    };
    let Some(row) = row else {
        return (false, "the input row is not on screen".into());
    };
    let grid_top = dev_rect(&probe(app, "ui.rect", json!({ "name": "grid" }))).map(|r| r.1);
    let d = probe(app, "dpi.state", Value::Null);
    let (Some(iy), Some(pad), Some(ch), Some(gy)) = (
        ime["y"].as_i64(),
        d["padding"].as_i64(),
        d["cell_h"].as_i64(),
        grid_top,
    ) else {
        return (false, "no rectangle to compare".into());
    };
    let top = gy + pad + row as i64 * ch;
    ((iy - top).abs() <= 2, format!("ime at {iy}, row top {top}"))
}

/// Whether the find bar casts a shadow: the ground just under the bar, against the grid's own
/// ground at the same height. None when the bar or a pixel cannot be read.
fn find_shadow(ctx: &mut Ctx, app: &mut App) -> Option<bool> {
    let (x, y, w, h) = dev_rect(&probe(app, "ui.rect", json!({ "name": "find" })))?;
    let (gx, _, _, _) = dev_rect(&probe(app, "ui.rect", json!({ "name": "grid" })))?;
    ctx.shot(app, "findbar")?;
    let below = y + h + 2;
    let under = pixel(app, x + w / 2, below)?;
    let ground = pixel(app, gx + 2, below)?;
    let diff = (0..3)
        .map(|i| (i32::from(under[i]) - i32::from(ground[i])).abs())
        .max()
        .unwrap_or(0);
    Some(diff > 2)
}

/// Clicks the menu row labelled `label`: the menu's box split into equal rows (the menu's own
/// rows are that tall). False when no such row is open.
fn click_menu_row(app: &mut App, label: &str) -> bool {
    let menu = probe(app, "app.state", Value::Null)["menu"]["rows"].clone();
    let labels: Vec<String> = menu
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| r["label"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let Some(idx) = labels.iter().position(|l| l == label) else {
        return false;
    };
    let Some((x, y, w, h)) = css_rect(&probe(app, "ui.rect", json!({ "name": "menu" }))) else {
        return false;
    };
    let n = labels.len() as f64;
    let at_y = y + (idx as f64 + 0.5) * h / n;
    app.session.click((x + w / 2.0) as f32, at_y as f32).is_ok()
}

/// Opens a fresh app with `opts`, runs `body`, and quits it whatever `body` did.
fn with_app(
    ctx: &mut Ctx,
    opts: Launch,
    body: impl FnOnce(&mut Ctx, &mut App) -> Result<(), String>,
) -> Result<(), String> {
    let mut app = ctx.launch(opts)?;
    // A step that could not run is a FAIL with its reason, always recorded (the runner records a
    // returned error only when no check failed before it).
    if let Err(e) = body(ctx, &mut app) {
        ctx.ok(false, format!("the scenario stopped: {e}"));
    }
    app.close();
    Ok(())
}

/// Writes the stand-in agent into `dir` and returns its path.
fn write_agent(dir: &Path) -> Result<PathBuf, String> {
    let path = dir.join("inline-agent.ps1");
    std::fs::write(&path, INLINE_AGENT).map_err(|e| format!("the stand-in agent: {e}"))?;
    Ok(path)
}

/// Whether `pwsh` is on this machine (PT's clickCaret runs in pwsh).
fn pwsh_present() -> bool {
    std::process::Command::new("where.exe")
        .arg("pwsh")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// PT: run.mjs:1200-1265. A stand-in agent streams above its input line; the textarea (the IME
/// rectangle) stays on the input line, and at a plain prompt it is back on the cursor.
pub fn caret_hold(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let agent = write_agent(&alpha)?;
    let opts = Launch {
        renderer: Some("software"),
        folders: vec![alpha],
        ..Launch::default()
    };
    with_app(ctx, opts, |ctx, app| caret_hold_run(ctx, app, &agent))
}

fn caret_hold_run(ctx: &mut Ctx, app: &mut App, agent: &Path) -> Result<(), String> {
    if !wait_prompt(app) {
        return Err("the shell showed no prompt".into());
    }
    run_line(
        app,
        &format!(
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -File '{}'",
            agent.display()
        ),
    )?;
    // PT: run.mjs:1234
    let streaming = wait_for("the stand-in agent streaming", 15_000, &mut || {
        text(app).contains("stream 3")
    });
    ctx.ok(
        streaming,
        "the stand-in agent is streaming above its input line",
    );
    for ch in "abc d".chars() {
        app.session.type_text(&ch.to_string())?;
        pause(220);
    }
    pause(600); // several frames, each parking the cursor eight rows up
    let g = grid(app);
    let input = row_of(&g, "INPUT> abc d");
    let parked = row_of(&g, "stream ");
    // PT: run.mjs:1242
    ctx.ok(
        matches!((parked, input), (Some(p), Some(i)) if p < i),
        format!("the program parks the cursor above the input line ({parked:?} < {input:?})"),
    );
    // PT: run.mjs:1243. Needs `ime.rect` (see ime_on_row).
    let (on, why) = ime_on_row(app, input);
    ctx.ok(
        on,
        format!("the textarea stays on the input line while it streams ({why})"),
    );
    app.session.type_text("e")?;
    pause(400);
    let input_again = row_of(&grid(app), "INPUT> abc de");
    // PT: run.mjs:1247
    let (on, why) = ime_on_row(app, input_again);
    ctx.ok(on, format!("and after the next key, still there ({why})"));
    app.session.type_text("q")?;
    // PT: run.mjs:1249-1250
    if !wait_prompt(app) {
        return Err(format!(
            "the stand-in agent did not end back at a prompt; the grid ends {:?}; its log ends {:?}",
            last_rows(app),
            log_tail(agent)
        ));
    }
    run_line(app, "echo caret-follows")?;
    let back = wait_for("back at a plain prompt", 8_000, &mut || {
        text(app).contains("caret-follows")
    });
    ctx.ok(back, "back at a plain prompt");
    pause(400);
    app.session.type_text("xyz")?;
    pause(400);
    // PT: run.mjs:1260. Needs `ime.rect` (see ime_on_row).
    let plain = prompt_row_with(&grid(app), "xyz");
    let (on, why) = ime_on_row(app, plain);
    ctx.ok(
        on,
        format!("at a plain prompt the textarea is on xterm's cursor, as before ({why})"),
    );
    app.session.key("Ctrl+C")
}

/// PT: run.mjs:1274-1326. A program that asks for the mouse gets the mouse: xterm takes the mode,
/// a key and a click reach it.
///
/// DROPPED: the bun variant of PT's loop (PT's sixth `ok(`, "bun is not installed here: skipped")
/// because Flare ships no bun case; the stand-in is a `.cmd` run by Windows PowerShell.
pub fn mouse_report(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let stand_in = alpha.join("mouse-probe.ps1");
    std::fs::write(&stand_in, MOUSE_PROBE).map_err(|e| format!("the mouse stand-in: {e}"))?;
    let opts = Launch {
        renderer: Some("software"),
        folders: vec![alpha],
        ..Launch::default()
    };
    with_app(ctx, opts, |ctx, app| mouse_report_run(ctx, app, &stand_in))
}

/// The REPORT lines on the front grid, the last three (what the probe received last).
fn last_reports(app: &mut App) -> String {
    let t = text(app);
    let mut lines: Vec<&str> = t.lines().filter(|l| l.contains("REPORT ")).collect();
    let n = lines.len();
    lines.drain(..n.saturating_sub(3));
    lines.join(" | ")
}

fn mouse_report_run(ctx: &mut Ctx, app: &mut App, stand_in: &Path) -> Result<(), String> {
    if !wait_prompt(app) {
        return Err("the shell showed no prompt".into());
    }
    run_line(
        app,
        &format!(
            "powershell.exe -NoProfile -ExecutionPolicy Bypass -File '{}'",
            stand_in.display()
        ),
    )?;
    // PT: run.mjs:1306 (node). The program has enabled mouse reporting.
    let up = wait_for("the program has asked for the mouse", 15_000, &mut || {
        mouse_on(app)
    });
    ctx.ok(up, "the program is up and has asked for the mouse");
    // PT: run.mjs:1309
    let mode = probe(app, "term.mouse", Value::Null);
    ctx.ok(mouse_on(app), format!("xterm took the mouse mode ({mode})"));
    // PT: run.mjs:1310. The pointer shape is set on the window (`appctl.rs` set_cursor) and no
    // probe reports it yet.
    ctx.ok(
        false,
        "so the pointer is not the text caret: the window exports no pointer shape to a probe yet",
    );
    // PT: run.mjs:1311-1312
    app.session.type_text("k")?;
    let key = wait_for("a key reaches it", 8_000, &mut || {
        text(app).contains("REPORT 107")
    });
    ctx.ok(key, "a key reaches it");
    // PT: run.mjs:1313-1319. A press (M) and a release (m) of an SGR report, `ESC [ < ...`.
    let (cx, cy) = grid_centre(app);
    app.session.click(cx, cy)?;
    let got = wait_for(
        "the click reaches it, press and release",
        5_000,
        &mut || {
            let t = text(app);
            t.contains("REPORT 60") && t.contains("REPORT 77") && t.contains("REPORT 109")
        },
    );
    let seen = last_reports(app);
    ctx.ok(
        got,
        format!(
            "the click reaches it, press and release (SGR '<', 'M' and 'm' reports; last: {seen})"
        ),
    );
    app.session.type_text("q")?;
    wait_prompt(app);
    Ok(())
}

/// PT: run.mjs:1327-1390. A click between two letters of the typed line puts the caret there; a
/// click on old output moves nothing. Needs pwsh; without it the check is a skip.
pub fn click_caret(ctx: &mut Ctx) -> Result<(), String> {
    if !pwsh_present() {
        ctx.skip("pwsh absent");
        return Ok(());
    }
    let alpha = ctx.ensure_world().alpha.clone();
    let opts = Launch {
        renderer: Some("software"),
        folders: vec![alpha],
        scale_dpi: Some(SCALE_225),
        ..Launch::default()
    };
    with_app(ctx, opts, click_caret_run)
}

fn click_caret_run(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    if !wait_prompt(app) {
        return Err("the shell showed no prompt".into());
    }
    let (gx, gy) = grid_centre(app);
    app.session.click(gx, gy)?;
    app.session.type_text("echo ab1cdef")?;
    pause(400);
    // PT: run.mjs:1365-1366
    let at = caret_point(app, "ab1cdef", 3);
    ctx.ok(at.is_some(), "the typed line is on screen");
    let (x, y) = at.unwrap_or((gx, gy));
    app.session.click(x, y)?;
    pause(300);
    app.session.type_text("X")?;
    app.session.key("Enter")?;
    // PT: run.mjs:1371-1374
    let moved = wait_for("the letter lands at the click", 8_000, &mut || {
        text(app).contains("ab1Xcdef")
    });
    ctx.ok(
        moved,
        "a click between two letters puts the caret there: the letter typed lands at the click",
    );
    // PT: run.mjs:1376-1377
    if !wait_prompt(app) {
        return Err("the shell showed no prompt after the edit".into());
    }
    app.session.type_text("echo zz")?;
    // PT: run.mjs:1378-1381
    let old = caret_point(app, "ab1Xcdef", 1);
    if let Some((x, y)) = old {
        app.session.click(x, y)?;
    }
    pause(300);
    app.session.type_text("Q")?;
    app.session.key("Enter")?;
    // PT: run.mjs:1383-1386
    let stayed = wait_for("the Q lands at the end", 8_000, &mut || {
        text(app).contains("zzQ")
    });
    ctx.ok(
        stayed,
        "a click on old output above the prompt moves nothing",
    );
    Ok(())
}

/// PT: run.mjs:1587-1617. Ctrl+F opens find at a prompt (no shadow, Escape closes it); in a
/// full-screen program Ctrl+F is the program's and Ctrl+Shift+F still finds. Runs at 225 percent.
pub fn find_key(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let opts = Launch {
        renderer: Some("software"),
        folders: vec![alpha],
        scale_dpi: Some(SCALE_225),
        ..Launch::default()
    };
    with_app(ctx, opts, find_key_run)
}

fn find_key_run(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    if !wait_prompt(app) {
        return Err("the shell showed no prompt".into());
    }
    let (gx, gy) = grid_centre(app);
    app.session.click(gx, gy)?;
    app.session.key("Ctrl+F")?;
    // PT: run.mjs:1595
    let opened = wait_for("find opens", 4_000, &mut || find_open(app));
    ctx.ok(opened, "Ctrl+F at the prompt opens find");
    // PT: run.mjs:1596-1597
    let shadow = find_shadow(ctx, app);
    ctx.ok(
        shadow == Some(false),
        format!("and the find box casts no shadow ({shadow:?})"),
    );
    app.session.key("Escape")?;
    // PT: run.mjs:1599
    let closed = wait_for("find closes", 4_000, &mut || !find_open(app));
    ctx.ok(closed, "and Escape closes it");
    // PT: run.mjs:1601-1604. The alternate screen, as vim and less switch to it.
    app.session.click(gx, gy)?;
    run_line(app, r#"Write-Host -NoNewline "$([char]27)[?1049h""#)?;
    pause(800);
    app.session.key("Ctrl+F")?;
    pause(600);
    // PT: run.mjs:1607
    ctx.ok(
        !find_open(app),
        "in a full-screen program Ctrl+F opens nothing: it is the program's",
    );
    app.session.key("Ctrl+Shift+F")?;
    // PT: run.mjs:1609
    let shift = wait_for("Ctrl+Shift+F finds there", 4_000, &mut || find_open(app));
    ctx.ok(shift, "while Ctrl+Shift+F still finds there");
    app.session.key("Escape")?;
    app.session.click(gx, gy)?;
    run_line(app, r#"Write-Host -NoNewline "$([char]27)[?1049l""#)
}

/// PT: run.mjs:1625-1685. The terminal's bar is 6 px wide, with no native gutter, and its thumb
/// is the text ink (lighter than the ground on a dark theme, darker on a light one). Runs at 225
/// percent. The GPU leg of PT's `[both]` is the orchestrator's (this group runs software only).
pub fn scrollbar(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let opts = Launch {
        renderer: Some("software"),
        folders: vec![alpha],
        scale_dpi: Some(SCALE_225),
        ..Launch::default()
    };
    with_app(ctx, opts, scrollbar_run)
}

/// Scrolls the grid up and back, which wakes xterm's own slider (it fades when idle, as VS Code's
/// does), then reads the bar's answer.
fn measure_bar(app: &mut App) -> Value {
    let (gx, gy) = grid_centre(app);
    // Positive dy is a scroll up (winit's convention); PT's wheel(0, -300) is up.
    let _ = app.session.call(&Request::Pointer {
        x: gx,
        y: gy,
        action: PointerAction::Wheel { dx: 0.0, dy: 300.0 },
    });
    pause(250);
    let _ = app.session.call(&Request::Pointer {
        x: gx,
        y: gy,
        action: PointerAction::Wheel {
            dx: 0.0,
            dy: -600.0,
        },
    });
    pause(400);
    probe(app, "scrollbar.rect", Value::Null)
}

fn scrollbar_run(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    if !wait_prompt(app) {
        return Err("the shell showed no prompt".into());
    }
    let (gx, gy) = grid_centre(app);
    app.session.click(gx, gy)?;
    run_line(app, "1..300")?;
    pause(1500);
    let dark = measure_bar(app);
    // PT: run.mjs:1670
    let gutter = dark["gutter"].as_i64();
    ctx.ok(
        gutter == Some(0),
        format!("no native gutter or steppers beside it ({gutter:?}px)"),
    );
    // PT: run.mjs:1671
    let width = dark["slider"]["w"].as_i64();
    ctx.ok(
        width == Some(6),
        format!("the slider is 6px wide ({width:?})"),
    );
    // PT: run.mjs:1672. The thumb against the ground, from pixels at the slider's centre.
    let thumb = slider_pixels(ctx, app, &dark);
    ctx.ok(
        matches!(thumb, Some((t, g)) if t - g > 25.0),
        format!("on a dark theme it is lighter than the ground ({thumb:?})"),
    );
    // PT: run.mjs:1673-1680 picks the light theme in Settings (Task 33's settings group owns that
    // path). Until that is reachable from this group, the two light checks read as failures.
    ctx.ok(
        false,
        "still 6px on a light theme: the light theme pick is not automated in this group (the Settings card helpers are private to settings_c and settings_d), and there is no bar to measure",
    );
    ctx.ok(
        false,
        "on a light theme it is darker than the ground: the light theme pick is not automated in this group (see the 6px check above)",
    );
    Ok(())
}

/// The luminance of the thumb and of the ground beside it (`40` device px left), when the bar has
/// a slider to sample.
fn slider_pixels(ctx: &mut Ctx, app: &mut App, bar: &Value) -> Option<(f64, f64)> {
    let s = &bar["slider"];
    let x = s["dx"].as_i64()? + s["dw"].as_i64()? / 2;
    let y = s["dy"].as_i64()? + s["dh"].as_i64()? / 2;
    ctx.shot(app, "scrollbar")?;
    let thumb = pixel(app, x, y)?;
    let ground = pixel(app, x - 40, y)?;
    Some((lum_of(thumb), lum_of(ground)))
}

/// The luminance of an sRGB colour on 0 to 255 (the Rec. 709 weights PT's screenshot sample uses).
fn lum_of(c: [u8; 3]) -> f64 {
    0.2126 * f64::from(c[0]) + 0.7152 * f64::from(c[1]) + 0.0722 * f64::from(c[2])
}

/// PT: run.mjs:1687-1752. Ctrl+V, Ctrl+Shift+V and the right-click Paste each deliver the copied
/// text exactly once. The clipboard is the in-memory stand-in: the OS clipboard is never touched.
pub fn paste(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let opts = Launch {
        renderer: Some("software"),
        folders: vec![alpha],
        ..Launch::default()
    };
    with_app(ctx, opts, paste_run)
}

fn paste_run(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    if !wait_prompt(app) {
        return Err("the shell showed no prompt".into());
    }
    mute_psreadline_paste(app);
    let (gx, gy) = grid_centre(app);
    app.session.click(gx, gy)?;
    // PT: run.mjs:1710. The clipboard is the in-memory stand-in under --e2e (`clip.last` answers).
    let stand_in = app.session.probe("clip.last", Value::Null).is_ok();
    ctx.ok(
        stand_in,
        "under --e2e the clipboard is a stand-in in memory",
    );

    // PT: run.mjs:1712-1715
    put_text(app, "PASTEONCEA")?;
    app.session.key("Ctrl+V")?;
    pause(900);
    let a = count(app, "PASTEONCEA");
    ctx.ok(
        a == 1,
        format!("Ctrl+V pastes the text ONCE ({a} copies arrived)"),
    );
    clear_line(app)?;

    // PT: run.mjs:1718-1721
    put_text(app, "PASTEONCEB")?;
    app.session.key("Ctrl+Shift+V")?;
    pause(900);
    let b = count(app, "PASTEONCEB");
    ctx.ok(
        b == 1,
        format!("Ctrl+Shift+V pastes the text ONCE ({b} copies arrived)"),
    );
    clear_line(app)?;

    // PT: run.mjs:1725-1729. The right-click menu's Paste reaches the same one paste.
    put_text(app, "PASTEONCEC")?;
    app.session.right_click(gx, gy)?;
    let menu_has_paste = wait_for("the menu has a Paste row", 4_000, &mut || {
        probe(app, "app.state", Value::Null)["menu"]["rows"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|r| r["label"] == json!("Paste")))
    });
    if menu_has_paste {
        click_menu_row(app, "Paste");
    }
    pause(900);
    let c = count(app, "PASTEONCEC");
    ctx.ok(
        c == 1,
        format!("the right-click Paste pastes ONCE ({c} copies arrived)"),
    );
    clear_line(app)
}
