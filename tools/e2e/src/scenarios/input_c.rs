//! Input C1: replies and clipboard scenarios (Task 31): termReplies, termClipboard, bell, hiddenResize.
//!
//! PT: run.mjs:3831-3930 (termReplies), 3932-4015 (termClipboard), 4125-4152 (bell), 4153-4214
//! (hiddenResize). Flare answers PT's page queries through `app.state` (the Copied badge and the
//! tabs), `grid.text` (the rows of the tab in front), `clip.last` (the clipboard), `picker.wall` and
//! `settings.theme` (the theme wall), `ui.rect` (the cog and the tabs), `flash.count` (the bell) and
//! `scrollbar.rect` (the slider). The stand-in programs write what they hear to a file the scenario
//! reads: `K <code>` for each key or reply byte, `W <cols>` for each width change, `R` once ready.
//! Printed as text, the replies would be read back by the terminal as escape sequences.
//!
//! DIFF-31a: Flare's XTVERSION answer is `ESC P > | Flare <version> (...) ESC \`, so the check counts
//! that prefix where PT counts its own full string.
//! DIFF-31b: the stand-in programs are PowerShell scripts (the same kind as Task 15's progs), not node.

use std::fs;
use std::path::{Path, PathBuf};

use flare_e2e_proto::{PointerAction, Request};
use serde_json::{json, Value};

use crate::ctx::{App, Ctx, Launch};
use crate::world::{prompt_showing, rows_text, Shell};

use super::driver::pick_theme;

/// A rectangle in css px: x, y, w, h.
type Rect4 = (f64, f64, f64, f64);

/// The ground PT Default answers OSC 11 with (ST-terminated).
const PT_DEFAULT_GROUND: &str = "\x1b]11;rgb:1717/1717/1717\x1b\\";
/// The ground Fawn answers OSC 11 with.
const FAWN_GROUND: &str = "\x1b]11;rgb:e6e6/d8d8/c0c0\x1b\\";
/// The dark and light reports of `CSI ? 996 n`.
const DARK: &str = "\x1b[?997;1n";
const LIGHT: &str = "\x1b[?997;2n";
/// The primary device attributes answer, sent last in stream order.
const DA1: &str = "\x1b[?1;2c";
/// DECRQM for mode 2031 once the program has set it.
const DECRQM_SET: &str = "\x1b[?2031;1$y";
/// Flare's XTVERSION answer starts with this; the version and the renderer follow.
const XTVERSION_PREFIX: &str = "\x1bP>|Flare ";

/// What a stand-in program logged: the bytes it was sent (one char per byte), the widths it saw
/// change, and whether it started.
#[derive(Debug, Default, PartialEq)]
struct Heard {
    ready: bool,
    bytes: String,
    widths: Vec<u32>,
}

/// Reads a stand-in's log: `R` (ready), `K <code>` (one byte it was sent), `W <cols>` (a width).
fn parse_heard(text: &str) -> Heard {
    let mut h = Heard::default();
    for line in text.lines() {
        if line == "R" {
            h.ready = true;
        } else if let Some(code) = line.strip_prefix("K ") {
            if let Some(c) = code.trim().parse::<u32>().ok().and_then(char::from_u32) {
                h.bytes.push(c);
            }
        } else if let Some(w) = line.strip_prefix("W ") {
            if let Ok(n) = w.trim().parse() {
                h.widths.push(n);
            }
        }
    }
    h
}

/// What the stand-in at `log` has logged so far (empty before it starts).
fn heard(log: &Path) -> Heard {
    parse_heard(&fs::read_to_string(log).unwrap_or_default())
}

/// The payload of the first OSC reply `ESC ] <code>` in `text`, up to its ST or BEL (`code` ends
/// with the semicolon, as in `10;`).
fn osc_payload(text: &str, code: &str) -> Option<String> {
    let start = text.find(&format!("\x1b]{code}"))? + 2 + code.len();
    let rest = &text[start..];
    let end = rest.find(['\x1b', '\x07']).unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// `s` as a PowerShell single-quoted literal.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Writes `<name>.ps1` (with `$e` the escape character and `$bel` the bell) and the `<name>.cmd`
/// that runs it under Windows PowerShell. Returns the `.cmd`, which is what the shell runs.
fn write_prog(dir: &Path, name: &str, body: &str) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("the program folder: {e}"))?;
    let script = format!("$e = [char]27\r\n$bel = [char]7\r\n{body}\r\n");
    fs::write(dir.join(format!("{name}.ps1")), script)
        .map_err(|e| format!("the program script: {e}"))?;
    let cmd = dir.join(format!("{name}.cmd"));
    let runner = format!(
        "@echo off\r\npowershell.exe -NoProfile -ExecutionPolicy Bypass -File \"%~dp0{name}.ps1\"\r\n"
    );
    fs::write(&cmd, runner).map_err(|e| format!("the program runner: {e}"))?;
    Ok(cmd)
}

/// The program that sends its queries and logs every byte it is sent (PT's replies probe). It
/// answers `t` with the ground and the dark or light report again, and logs each width change.
fn replies_program(dir: &Path, log: &Path) -> Result<PathBuf, String> {
    let body = r#"$log = __LOG__
function Note([string]$s) { [IO.File]::AppendAllText($log, $s + "`r`n") }
[IO.File]::WriteAllText($log, '')
Note 'R'
[Console]::Write($e + '[?2031h' + $e + ']11;?' + $bel + $e + ']10;?' + $bel + $e + '[?996n' + $e + '[?2031$p' + $e + '[>q' + $e + '[>0q' + $e + '[>1q' + $e + '[c')
$w = [Console]::WindowWidth
while ($true) {
  if ([Console]::KeyAvailable) {
    $k = [Console]::ReadKey($true)
    Note ('K ' + [int]$k.KeyChar)
    if ($k.KeyChar -eq 'q') { break }
    if ($k.KeyChar -eq 't') { [Console]::Write($e + ']11;?' + $bel + $e + '[?996n') }
  } else {
    $nw = [Console]::WindowWidth
    if ($nw -ne $w) { Note ('W ' + $nw); $w = $nw }
    Start-Sleep -Milliseconds 30
  }
}"#;
    write_prog(
        dir,
        "replies-probe",
        &body.replace("__LOG__", &ps_quote(&log.to_string_lossy())),
    )
}

/// The program that writes the clipboard: `1` an OSC 52 write (UTF-8), `2` an OSC 52 read, `3` an
/// OSC 52 write to the `p` selection. It logs every byte it is sent.
fn clipboard_program(dir: &Path, log: &Path) -> Result<PathBuf, String> {
    let body = r#"$log = __LOG__
function Note([string]$s) { [IO.File]::AppendAllText($log, $s + "`r`n") }
[IO.File]::WriteAllText($log, '')
Note 'R'
$b64 = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes('OSC52 ' + [char]0x00e6))
$psel = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes('PSEL52'))
while ($true) {
  $k = [Console]::ReadKey($true)
  Note ('K ' + [int]$k.KeyChar)
  if ($k.KeyChar -eq 'q') { break }
  if ($k.KeyChar -eq '1') { [Console]::Write($e + ']52;c;' + $b64 + $bel) }
  if ($k.KeyChar -eq '2') { [Console]::Write($e + ']52;c;?' + $bel) }
  if ($k.KeyChar -eq '3') { [Console]::Write($e + ']52;p;' + $psel + $bel) }
}"#;
    write_prog(
        dir,
        "osc52-probe",
        &body.replace("__LOG__", &ps_quote(&log.to_string_lossy())),
    )
}

/// The program that holds the alternate screen with mouse tracking on and draws one `ALTROW n of m`
/// line per row, again on every resize (PT's alt-probe). `q` leaves the alternate screen.
fn alt_program(dir: &Path) -> Result<PathBuf, String> {
    let body = r#"function Draw {
  $r = [Console]::WindowHeight
  $c = [Console]::WindowWidth
  $s = $e + '[H'
  for ($i = 1; $i -le $r; $i++) {
    $t = 'ALTROW ' + $i + ' of ' + $r
    if ($t.Length -gt ($c - 1)) { $t = $t.Substring(0, $c - 1) } else { $t = $t.PadRight($c - 1) }
    $s += $t
    if ($i -lt $r) { $s += "`r`n" }
  }
  [Console]::Write($s)
}
[Console]::Write($e + '[?1049h' + $e + '[?1000h' + $e + '[?1006h' + $e + '[2J')
Draw
$w = [Console]::WindowWidth
$h = [Console]::WindowHeight
while ($true) {
  if ([Console]::KeyAvailable) {
    if ([Console]::ReadKey($true).KeyChar -eq 'q') { break }
  } else {
    if ([Console]::WindowWidth -ne $w -or [Console]::WindowHeight -ne $h) {
      $w = [Console]::WindowWidth
      $h = [Console]::WindowHeight
      Draw
    }
    Start-Sleep -Milliseconds 30
  }
}
[Console]::Write($e + '[?1006l' + $e + '[?1000l' + $e + '[?1049l')"#;
    write_prog(dir, "alt-probe", body)
}

/// The probe value of `name` (Null when the probe does not answer).
fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// A `{x, y, w, h}` JSON rectangle, or None.
fn rect_of(v: &Value) -> Option<Rect4> {
    Some((
        v["x"].as_f64()?,
        v["y"].as_f64()?,
        v["w"].as_f64()?,
        v["h"].as_f64()?,
    ))
}

/// The centre of a rectangle, as the pointer wants it.
fn centre(r: Rect4) -> (f32, f32) {
    ((r.0 + r.2 / 2.0) as f32, (r.1 + r.3 / 2.0) as f32)
}

/// The named rectangle the window exports (`ui.rect`).
fn ui_rect(app: &mut App, name: &str) -> Option<Rect4> {
    rect_of(&probe(app, "ui.rect", json!({ "name": name })))
}

/// Clicks the centre of the named rectangle; false when it is not drawn.
fn click_name(app: &mut App, name: &str) -> bool {
    match ui_rect(app, name) {
        Some(r) => {
            let (x, y) = centre(r);
            app.session.click(x, y).is_ok()
        }
        None => false,
    }
}

/// The centre of the window's grid (where the pane's wheel goes), in client px.
fn grid_centre(app: &mut App) -> (f32, f32) {
    ui_rect(app, "grid").map_or((400.0, 300.0), centre)
}

/// One wheel step at a point (`dy` negative scrolls down).
fn wheel(app: &mut App, x: f32, y: f32, dy: f32) {
    let _ = app.session.call(&Request::Pointer {
        x,
        y,
        action: PointerAction::Wheel { dx: 0.0, dy },
    });
}

/// The tabs the app shows (`app.state`), as their count.
fn tab_count(app: &mut App) -> usize {
    app.session.state().map(|s| s.tabs.len()).unwrap_or(0)
}

/// The id of the tab in front ("" when the probe does not answer).
fn front_tab(app: &mut App) -> String {
    app.session
        .state()
        .ok()
        .and_then(|s| s.front)
        .unwrap_or_default()
}

/// The text of the tab in front, one row per line.
fn front_rows(app: &mut App) -> String {
    let tab = front_tab(app);
    rows_text(app, &tab)
}

/// Types one line into the front shell and presses Enter (PT `typeLine`).
fn type_line(app: &mut App, line: &str) -> Result<(), String> {
    app.session.type_text(line)?;
    app.session.key("Enter")
}

/// Types a marker line until the shell runs it (a fresh PowerShell drops what is typed before its
/// line editor is up). True once the marker's answer shows.
fn shell_ready(ctx: &mut Ctx, app: &mut App) -> bool {
    for _ in 0..20 {
        // Escape throws away whatever half a line an earlier try left.
        let _ = app.session.key("Escape");
        let _ = type_line(app, "echo rdy-$(1+1)");
        if ctx.poll(2_000, &mut || front_rows(app).contains("rdy-2")) {
            return true;
        }
    }
    false
}
/// Holds for `ms` in 50 ms steps: a "nothing happens" proof, as PT's sleeps are.
fn hold(ctx: &mut Ctx, ms: u64) {
    for _ in 0..(ms / 50) {
        ctx.settle(50);
    }
}

/// How many times `needle` occurs in `text`.
fn count(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

/// Brings the first tab (the shell the scenario typed into) to the front.
fn back_to_first(ctx: &mut Ctx, app: &mut App) {
    let first = app
        .session
        .state()
        .ok()
        .and_then(|s| s.tabs.first().map(|t| t.id.clone()))
        .unwrap_or_default();
    click_name(app, "tab:0");
    ctx.until("the shell tab in front", 5_000, &mut || {
        front_tab(app) == first
    });
}

/// PT: run.mjs:3831-3930. The stand-in program is sent its replies in stream order (DA1 last),
/// the ground and text colours, the dark report, the mode 2031 report, and XTVERSION three ways.
/// Picking Fawn tells the program light once and nudges its width one column and back once;
/// asking again answers Fawn's ground; PT Default tells it dark again.
pub fn term_replies(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let log = alpha.join("replies.log");
    let cmd = replies_program(&alpha, &log)?;
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha.clone()],
        ..Launch::default()
    })?;
    ctx.until("the first tab", 15_000, &mut || tab_count(&mut app) == 1);
    ctx.until("the first prompt", 45_000, &mut || {
        prompt_showing(&front_rows(&mut app), Shell::PowerShell)
    });
    type_line(&mut app, &format!("& {}", ps_quote(&cmd.to_string_lossy())))?;

    let answered = ctx.until(
        "the stand-in program heard its DA1 answer",
        15_000,
        &mut || heard(&log).bytes.contains(DA1),
    );
    ctx.ok(answered, "the stand-in program heard its DA1 answer");
    let first = heard(&log).bytes;
    ctx.ok(
        first.contains(PT_DEFAULT_GROUND),
        format!(
            "OSC 11 answers PT Default's ground, not black ({:?})",
            osc_payload(&first, "11;")
        ),
    );
    ctx.ok(
        !first.contains("rgb:0000/0000/0000"),
        "and nothing answers transparent black",
    );
    let ink = osc_payload(&first, "10;")
        .unwrap_or_default()
        .replace("\x1b", "");
    ctx.ok(
        ink.starts_with("rgb:") && ink != "rgb:1717/1717/1717",
        format!("OSC 10 answers the theme's text ({ink})"),
    );
    ctx.ok(
        first.contains(DARK),
        "CSI ? 996 n answers dark on PT Default",
    );
    ctx.ok(
        first.contains(DECRQM_SET),
        "DECRQM says 2031 is set once the program set it",
    );
    ctx.ok(
        first.rfind(DA1) > first.rfind(DECRQM_SET),
        "and the DA1 answer comes LAST: the replies keep stream order",
    );
    let xt = count(&first, XTVERSION_PREFIX);
    ctx.ok(
        xt == 2,
        format!("XTVERSION answers CSI > q and CSI > 0 q, and not CSI > 1 q ({xt} of {XTVERSION_PREFIX:?})"),
    );
    ctx.ok(
        count(&first, "\x1bP>|") == 2,
        "and nothing else answers as a version",
    );
    ctx.ok(
        first.rfind(DA1) > first.rfind(XTVERSION_PREFIX),
        "the version answers come before DA1, in stream order",
    );

    // The theme is switched on the real wall, by pointer, never by a write probe.
    let _ = pick_theme(ctx, &mut app, "fawn");
    back_to_first(ctx, &mut app);
    let light = ctx.until("the program told light", 5_000, &mut || {
        count(&heard(&log).bytes, LIGHT) >= 1
    });
    ctx.ok(light, "picking Fawn tells the program light");
    hold(ctx, 800);
    let lights = count(&heard(&log).bytes, LIGHT);
    ctx.ok(lights == 1, format!("exactly once ({lights})"));

    // THE REDRAW NUDGE (owner, 2026-10-11): once the switch is made the program sees one column
    // less, then its width back, and only once.
    ctx.until("two width changes", 5_000, &mut || {
        heard(&log).widths.len() >= 2
    });
    let widths = heard(&log).widths;
    let n = widths.len();
    ctx.ok(
        n >= 2 && widths[n - 2] + 1 == widths[n - 1],
        format!("the program is nudged one column narrower and back ({widths:?})"),
    );
    ctx.ok(n == 2, format!("once, for one switch ({n} resizes)"));

    app.session.type_text("t")?;
    let fawn = ctx.until("OSC 11 answers Fawn's ground", 5_000, &mut || {
        heard(&log).bytes.contains(FAWN_GROUND)
    });
    ctx.ok(fawn, "asked again, OSC 11 answers Fawn's ground");

    let _ = pick_theme(ctx, &mut app, "pt-default");
    back_to_first(ctx, &mut app);
    let dark = ctx.until("the program told dark again", 5_000, &mut || {
        count(&heard(&log).bytes, DARK) >= 2
    });
    ctx.ok(dark, "back to PT Default tells the program dark");
    // DROPPED: "a font size change sends the program nothing" because the terminal font size is a
    // dropdown whose options no probe reads (settings_b keeps the same skip for it).
    ctx.skip("DROPPED: a font size change sends nothing (the font size menu has no probe to pick an option)");

    app.session.type_text("q")?;
    app.close();
    Ok(())
}

/// PT: run.mjs:3932-4015. OSC 52 is write-only (#176): a write lands on the clipboard with its UTF-8
/// intact and shows the Copied badge without being drawn; a read gets no reply and leaves the
/// clipboard alone; a `p` selection changes nothing.
pub fn term_clipboard(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let log = alpha.join("osc52.log");
    let cmd = clipboard_program(&alpha, &log)?;
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha.clone()],
        ..Launch::default()
    })?;
    ctx.until("the first tab", 15_000, &mut || tab_count(&mut app) == 1);
    ctx.until("the first prompt", 45_000, &mut || {
        prompt_showing(&front_rows(&mut app), Shell::PowerShell)
    });
    let _ = app.session.call(&Request::ClipSet {
        text: Some("BEFORE-52".to_string()),
        png_b64: None,
    });
    type_line(&mut app, &format!("& {}", ps_quote(&cmd.to_string_lossy())))?;
    let running = ctx.until("the stand-in program is running", 15_000, &mut || {
        heard(&log).ready
    });
    ctx.ok(running, "the stand-in program is running");

    app.session.type_text("1")?;
    let landed = ctx.until("an OSC 52 write lands on the clipboard", 5_000, &mut || {
        clip_text(&mut app) == "OSC52 \u{e6}"
    });
    ctx.ok(
        landed,
        format!(
            "a program's OSC 52 write lands on the clipboard, UTF-8 intact ({:?})",
            clip_text(&mut app)
        ),
    );
    let badge = ctx.until("the Copied badge", 3_000, &mut || badge_shown(&mut app));
    ctx.ok(badge, "and the \"Copied\" badge says so");
    ctx.ok(
        !front_rows(&mut app).contains("]52;"),
        "nothing of the sequence is drawn",
    );

    let _ = app.session.call(&Request::ClipSet {
        text: Some("HELD-52".to_string()),
        png_b64: None,
    });
    app.session.type_text("2")?;
    ctx.until("the read request reached the program", 5_000, &mut || {
        heard(&log).bytes.ends_with('2')
    });
    hold(ctx, 1_000);
    ctx.ok(
        !heard(&log).bytes.contains("\x1b]52;"),
        "a READ gets no reply: the program hears nothing",
    );
    ctx.ok(
        clip_text(&mut app) == "HELD-52",
        "and the clipboard is left as it was",
    );

    app.session.type_text("3")?;
    ctx.until(
        "the selection request reached the program",
        5_000,
        &mut || heard(&log).bytes.ends_with('3'),
    );
    hold(ctx, 800);
    ctx.ok(
        clip_text(&mut app) == "HELD-52",
        "a \"p\" selection changes nothing",
    );

    app.session.type_text("q")?;
    app.close();
    Ok(())
}

/// The clipboard's text (`clip.last`), empty when there is none.
fn clip_text(app: &mut App) -> String {
    probe(app, "clip.last", Value::Null)["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Whether the Copied badge is on screen (`app.state` badge).
fn badge_shown(app: &mut App) -> bool {
    probe(app, "app.state", Value::Null)["badge"]["visible"] == json!(true)
}

/// The bell flash count (`flash.count`), -1 when the probe does not answer.
fn flash_count(app: &mut App) -> i64 {
    probe(app, "flash.count", Value::Null)["count"]
        .as_i64()
        .unwrap_or(-1)
}

/// PT: run.mjs:4125-4152. A bell flashes the taskbar once while the window is unfocused; ten in a
/// burst flash once (at most one a second per tab).
pub fn bell(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha],
        ..Launch::default()
    })?;
    let prompt = ctx.until("the first prompt", 45_000, &mut || {
        prompt_showing(&front_rows(&mut app), Shell::PowerShell)
    });
    if !prompt {
        return Err("the PowerShell prompt did not come back".into());
    }
    // A fresh PowerShell shows its prompt before PSReadLine is reading, and a line typed then is
    // dropped (the first characters, and the Enter). Typed lines go in only once one has run.
    let ready = shell_ready(ctx, &mut app);
    ctx.ok(ready, "the shell runs a typed line");
    let before = flash_count(&mut app);
    ctx.ok(before == 0, format!("no flash before a bell ({before})"));

    type_line(&mut app, "[Console]::Write([char]7)")?;
    let one = ctx.until("one flash", 5_000, &mut || flash_count(&mut app) == 1);
    ctx.ok(
        one,
        format!("one bell, one flash ({})", flash_count(&mut app)),
    );
    hold(ctx, 1_300); // past the one-second gate
    type_line(
        &mut app,
        "1..10 | % { [Console]::Write([char]7); Start-Sleep -Milliseconds 20 }",
    )?;
    hold(ctx, 1_500);
    let burst = flash_count(&mut app);
    ctx.ok(
        burst == 2,
        format!("ten bells in a burst flash once ({})", burst - 1),
    );
    app.close();
    Ok(())
}

/// The `ALTROW n of m` numbers of a line of the alternate-screen frame, or None.
fn parse_altrow(line: &str) -> Option<(u32, u32)> {
    let rest = line.trim_end().strip_prefix("ALTROW ")?;
    let mut parts = rest.split_whitespace();
    let n = parts.next()?.parse().ok()?;
    if parts.next()? != "of" {
        return None;
    }
    let m = parts.next()?.parse().ok()?;
    Some((n, m))
}

/// True when the tab shows row 1 of its frame at the top.
fn has_row_one(text: &str) -> bool {
    text.lines()
        .any(|l| parse_altrow(l).is_some_and(|(n, _)| n == 1))
}

/// True when the frame is complete for its size: as many `ALTROW` rows as the last one says, and
/// the last one is the last of them.
fn full_frame(text: &str) -> bool {
    let frame: Vec<(u32, u32)> = text.lines().filter_map(parse_altrow).collect();
    match frame.last() {
        Some(&(n, m)) => n == m && frame.len() == m as usize,
        None => false,
    }
}

/// PT: run.mjs:4153-4214. A tab resized while hidden comes back with its frame drawn for the new
/// size, and nothing above the frame to scroll to.
pub fn hidden_resize(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, beta) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), w.beta.clone())
    };
    let cmd = alt_program(&beta)?;
    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha, beta],
        ..Launch::default()
    })?;
    let two = ctx.until("two tabs open", 15_000, &mut || tab_count(&mut app) == 2);
    ctx.ok(two, "two tabs open");

    // Tab B (beta) is the one the program runs in.
    click_name(&mut app, "tab:1");
    ctx.until("the prompt on tab B", 45_000, &mut || {
        prompt_showing(&front_rows(&mut app), Shell::PowerShell)
    });
    type_line(&mut app, &format!("& {}", ps_quote(&cmd.to_string_lossy())))?;
    let drawn = ctx.until("tab B on the alternate screen", 15_000, &mut || {
        has_row_one(&front_rows(&mut app))
    });
    ctx.ok(drawn, "tab B is on the alternate screen, every row drawn");

    // Tab B hidden while the window shrinks by 300 px each way.
    click_name(&mut app, "tab:0");
    hold(ctx, 500);
    let st = probe(&mut app, "app.state", Value::Null);
    let k = st["scale"].as_f64().unwrap_or(1.0).max(0.1);
    let cw = st["window"]["client_w"].as_f64().unwrap_or(1100.0 * k) / k;
    let ch = st["window"]["client_h"].as_f64().unwrap_or(700.0 * k) / k;
    let _ = app.session.call(&Request::Resize {
        w: (cw - 300.0) as f32,
        h: (ch - 300.0) as f32,
    });
    hold(ctx, 1_000);
    click_name(&mut app, "tab:1");
    let _ = app.session.call(&Request::Pointer {
        x: 400.0,
        y: 300.0,
        action: PointerAction::Move,
    });
    let full = ctx.until("a full frame for the new size", 10_000, &mut || {
        full_frame(&front_rows(&mut app))
    });
    ctx.ok(
        full,
        "back on tab B, the program drew a full frame for the new size",
    );
    hold(ctx, 600);

    // DROPPED: "the slider is hidden or fills its track". The window draws no slider at all
    // (scrollbar.rect answers slider null), so there is no scroll range that could go stale.
    ctx.skip("DROPPED: the scroll slider is hidden or fills its track (the window draws no slider; scrollbar.rect answers slider null)");

    let (x, y) = grid_centre(&mut app);
    wheel(&mut app, x, y, -200.0);
    hold(ctx, 300);
    let top = front_rows(&mut app)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    ctx.ok(
        top.starts_with("ALTROW 1 of"),
        format!(
            "and nothing above the frame to scroll to (top row \"{}\")",
            top.chars().take(20).collect::<String>()
        ),
    );

    app.session.type_text("q")?;
    let _ = app.session.call(&Request::Resize {
        w: 1100.0,
        h: 700.0,
    });
    app.close();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stand_in_log_gives_its_ready_flag_bytes_and_widths() {
        let h = parse_heard("R\r\nK 27\r\nK 91\r\nK 63\r\nW 99\r\nK 113\r\n");
        assert!(h.ready);
        assert_eq!(h.bytes, "\u{1b}[?q");
        assert_eq!(h.widths, vec![99]);
        assert_eq!(parse_heard(""), Heard::default());
    }

    #[test]
    fn an_osc_payload_stops_at_its_st_or_bel() {
        assert_eq!(
            osc_payload("x\x1b]10;rgb:e6e6/d8d8/c0c0\x1b\\y", "10;").as_deref(),
            Some("rgb:e6e6/d8d8/c0c0")
        );
        assert_eq!(
            osc_payload("\x1b]11;rgb:1/2/3\x07", "11;").as_deref(),
            Some("rgb:1/2/3")
        );
        assert_eq!(osc_payload("nothing here", "11;"), None);
    }

    #[test]
    fn a_single_quote_is_doubled_in_a_script_path() {
        assert_eq!(ps_quote("C:\\it's"), "'C:\\it''s'");
    }

    #[test]
    fn the_replies_program_logs_to_its_own_file_and_is_a_runnable_pair() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("replies.log");
        let cmd = replies_program(dir.path(), &log).unwrap();
        assert!(cmd.exists());
        let ps1 = fs::read_to_string(dir.path().join("replies-probe.ps1")).unwrap();
        assert!(ps1.contains(&ps_quote(&log.to_string_lossy())));
        assert!(!ps1.contains("__LOG__"));
        assert!(ps1.contains("?2031h") && ps1.contains("[c"));
        let runner = fs::read_to_string(&cmd).unwrap();
        assert!(runner.contains("replies-probe.ps1"));
    }

    #[test]
    fn the_clipboard_program_writes_utf8_base64_and_never_a_read_reply() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("osc52.log");
        clipboard_program(dir.path(), &log).unwrap();
        let ps1 = fs::read_to_string(dir.path().join("osc52-probe.ps1")).unwrap();
        assert!(ps1.contains("UTF8.GetBytes"));
        assert!(ps1.contains("]52;p;"), "the p selection request");
        assert!(ps1.contains("]52;c;?"), "the read request");
    }

    #[test]
    fn an_altrow_line_gives_its_numbers() {
        assert_eq!(parse_altrow("ALTROW 3 of 40   "), Some((3, 40)));
        assert_eq!(parse_altrow("ALTROW x of 40"), None);
        assert_eq!(parse_altrow("something else"), None);
    }

    #[test]
    fn a_full_frame_has_every_row_and_ends_on_the_last() {
        let whole: String = (1..=3).map(|i| format!("ALTROW {i} of 3\n")).collect();
        assert!(full_frame(&whole));
        assert!(has_row_one(&whole));
        let stale = "ALTROW 1 of 5\nALTROW 2 of 5\nALTROW 3 of 5\n";
        assert!(!full_frame(stale));
        assert!(has_row_one(stale));
        assert!(!full_frame(""));
        assert!(!has_row_one("ALTROW 2 of 3\n"));
    }
}
