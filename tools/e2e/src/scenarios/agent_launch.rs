//! Agent launch, Sonara and image paste scenarios (Task 28): launchSkeleton, sonaraQuestion, imagePaste.
//!
//! PT: run.mjs:3279-3377 (launchSkeleton, 9 checks), 3756-3830 (sonaraQuestion, 10), 4016-4124
//! (imagePaste, 14). Prism Terminal reads its DOM; Flare answers through `app.state` (the tab rows,
//! their marks and rects), `agent.state` (presence and the poll's verdict), `launch.state` (the
//! loading rings, e2e_probes/agent_launch.rs), `grid.text` (the shell's text), the Sonara stand-in's
//! posts and the in-memory clipboard.
//!
//! DROPPED (each one is a `skip` where PT checks it): the first frame holding the tabs (no probe
//! reads the first painted frame), the resume skeleton's flag (an input property of the window that
//! no probe reads back; the skeleton-part of two checks is dropped with it, and says so where it is
//! met), the loading ring's place at the tab's end (a layout fact the tab rows do not report) and the
//! bare-ground sample while the skeleton covers the grid (the cover is pixels).
//!
//! Stand-ins: a Claude is a `FakeClaude` (`claude` resolves to its `claude.cmd` on the PATH the launch
//! sets). PT's key-dump program is a raw-mode node script; Flare's is PowerShell reading keys with
//! `Console.ReadKey`, so it prints each key's character in hex, an Alt key as `1b` then the character.
//! That is the byte PT sees for the keys these scenarios send (ESC v, ^V, ^D, ^C, CR, ^J).

use std::path::Path;

use flare_e2e_proto::{Reply, Request};
use serde_json::{json, Value};

use crate::ctx::{App, Ctx, Launch};
use crate::standins::agent_cmd::{FakeClaude, Step};
use crate::standins::sonara::{Post, SonaraStandIn};
use crate::until::{wait_on, SystemClock};
use crate::world::{mute_psreadline_paste, prompt_timeout, rows_text, wait_for_prompt, Shell};

/// The session id every restored folder's transcript carries (PT's `5a1d0c2e-...`).
const SESSION: &str = "5a1d0c2e-1111-4222-8333-94445555a666";
/// The bearer key runtime.json carries (PT's `e2e-token`).
const SONARA_KEY: &str = "e2e-token";
/// The one path Sonara answers.
const QUESTION_PATH: &str = "/v1/question_shown";
/// The words the shell prints before the agent takes the console (PT's `NOISE-BEFORE`).
const NOISE: &str = "NOISE-BEFORE";
/// The line the stand-in prints once it is ready (PT's `FAKE CLAUDE READY`).
const READY: &str = "FAKE CLAUDE READY";
/// The four screens of the question widget, as PT records them (tools/e2e/fixtures).
const QUESTION_SCREENS: &str = include_str!("../../fixtures/question_screens.json");
/// The UTF-8 byte-order mark Windows PowerShell reads a script's non-ASCII characters by.
const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The probe's answer for `name`, `Null` when it fails (a check on it then reads false).
fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// The `app.state` answer (tabs with their ids, marks and rects).
fn app_state(app: &mut App) -> Value {
    probe(app, "app.state", Value::Null)
}

/// How many tabs the app shows now.
fn tab_count(app: &mut App) -> usize {
    app_state(app)["tabs"].as_array().map_or(0, Vec::len)
}

/// The id of the tab at `index` in strip order, if there is one.
fn tab_id_at(app: &mut App, index: usize) -> Option<String> {
    app_state(app)["tabs"][index]["id"]
        .as_str()
        .map(str::to_string)
}

/// The id of the tab in front, as the session reports it.
fn front_tab(app: &mut App) -> String {
    app.session
        .state()
        .ok()
        .and_then(|s| s.front)
        .unwrap_or_default()
}

/// `agent.state`'s tabs.
fn agents(app: &mut App) -> Vec<Value> {
    probe(app, "agent.state", Value::Null)["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// True when some tab hosts a present agent (PT's `[data-agent-present]`).
fn any_agent_present(app: &mut App) -> bool {
    agents(app).iter().any(|t| t["present"] == json!(true))
}

/// True once the process poll has given its verdict for every tab.
fn polled(app: &mut App) -> bool {
    probe(app, "agent.state", Value::Null)["polled"] == json!(true)
}

/// The loading rings the window shows, and the shell text of the tab `tab` (the grid's rows as one
/// string, NBSP read as a space).
fn look(app: &mut App, tab: &str) -> (u64, String) {
    let rings = probe(app, "launch.state", Value::Null)["rings"]
        .as_u64()
        .unwrap_or(0);
    (rings, text_of(app, tab))
}

/// The shell text of tab `tab`, rows joined by newlines.
fn text_of(app: &mut App, tab: &str) -> String {
    rows_text(app, tab).replace('\u{a0}', " ")
}

/// The shell text of the tab in front.
fn front_text(app: &mut App) -> String {
    let front = front_tab(app);
    text_of(app, &front)
}

/// Waits up to `ms` for `cond`, polling every 25 ms. Records nothing on a timeout: the caller's
/// `ctx.ok` says what was missing.
fn wait(ms: u64, cond: &mut dyn FnMut() -> bool) -> bool {
    wait_on(&mut SystemClock::new(), ms, cond)
}

/// Holds the scenario still for `ms` (a "nothing changed" proof), in steps the scan allows.
fn quiet(ctx: &mut Ctx, ms: u64) {
    for _ in 0..ms / 50 {
        ctx.settle(50);
    }
}

/// True when `text` holds `token` as a whole word: the character after it is not a letter or digit
/// (PT's `/IN C 16\b/`).
fn has_token(text: &str, token: &str) -> bool {
    count_tokens(text, token) > 0
}

/// How many times `token` appears as a whole word in `text`.
fn count_tokens(text: &str, token: &str) -> usize {
    text.match_indices(token)
        .filter(|(i, _)| {
            !text[i + token.len()..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric)
        })
        .count()
}

/// Types a line into the shell in front once it shows its prompt.
fn type_line(app: &mut App, line: &str) {
    let front = front_tab(app);
    let _ = wait_for_prompt(
        app,
        &front,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    );
    let _ = app.session.type_text(line);
    let _ = app.session.key("Enter");
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

/// Writes `body` as a UTF-8 file with a byte-order mark, so Windows PowerShell reads it as written.
fn write_script(path: &Path, body: &str) -> Result<(), String> {
    let bytes = [BOM, body.as_bytes()].concat();
    std::fs::write(path, bytes).map_err(|e| format!("write {}: {e}", path.display()))
}

/// The key-dump program: `READY <tag>` once, then one `IN <tag> <hex>` line per key read (an Alt key
/// is `1b` then its character), `t` titles it as Claude and prints `TITLED`, and `q` leaves. The tag
/// is its first argument.
fn key_dump_script() -> String {
    "[Console]::OutputEncoding = [Text.Encoding]::UTF8
[Console]::TreatControlCAsInput = $true
$tag = if ($args.Count -gt 0) { $args[0] } else { '?' }
[Console]::Write('READY ' + $tag + [char]13 + [char]10)
while ($true) {
  $k = [Console]::ReadKey($true)
  $c = [string]$k.KeyChar
  $alt = ($k.Modifiers -band [ConsoleModifiers]::Alt) -ne 0
  if (-not $alt -and $c -eq 'q') { exit 0 }
  if (-not $alt -and $c -eq 't') {
    [Console]::Write([string][char]27 + ']0;' + [string][char]0x2733 + ' Claude Code' + [string][char]7 + 'TITLED' + [char]13 + [char]10)
    continue
  }
  $hex = '{0:x2}' -f [int][char]$c
  if ($alt) { $hex = '1b ' + $hex }
  [Console]::Write('IN ' + $tag + ' ' + $hex + [char]13 + [char]10)
}
"
    .to_string()
}

/// The question widget's screens, from the fixture: each screen as its lines joined with CRLF.
fn question_screens() -> Result<Vec<String>, String> {
    let screens: Vec<Vec<String>> = serde_json::from_str(QUESTION_SCREENS)
        .map_err(|e| format!("the question screens fixture: {e}"))?;
    Ok(screens.iter().map(|lines| lines.join("\r\n")).collect())
}

/// The question program: draws the screen it is on at once, `n` moves to the next screen, any other
/// key (`x`) redraws the same one, and `q` leaves.
fn question_script(screens: &[String]) -> String {
    let list = screens
        .iter()
        .map(|s| format!("'{}'", s.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",\r\n  ");
    format!(
        "[Console]::OutputEncoding = [Text.Encoding]::UTF8\r\n\
         $screens = @(\r\n  {list}\r\n)\r\n\
         $k = 0\r\n\
         function Draw {{ [Console]::Write([string][char]27 + '[2J' + [string][char]27 + '[H' + $screens[$k]) }}\r\n\
         Draw\r\n\
         while ($true) {{\r\n\
         \x20 $key = [Console]::ReadKey($true)\r\n\
         \x20 if ([string]$key.KeyChar -eq 'q') {{ exit 0 }}\r\n\
         \x20 if ([string]$key.KeyChar -eq 'n' -and $k -lt $screens.Count - 1) {{ $k = $k + 1 }}\r\n\
         \x20 Draw\r\n\
         }}\r\n"
    )
}

/// The `claude.cmd` that runs a PowerShell script with the arguments it was given (the tag).
const CLAUDE_CMD_FORWARDING: &str = "@echo off\r\npowershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File \"%~dp0claude-steps.ps1\" %*\r\n";

/// The `claude.cmd` that prints the shell's words first, then runs the stand-in (as PT's does).
const CLAUDE_CMD_NOISY: &str = "@echo NOISE-BEFORE C:\\noise\r\n@powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File \"%~dp0claude-steps.ps1\" %*\r\n";

/// The command that runs a script in the shell, with its arguments.
fn run_script_line(script: &Path, args: &str) -> String {
    format!(
        "powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File '{}' {args}",
        script.display()
    )
}

/// Standard base64 (RFC 4648) of `bytes`, for the clipboard's PNG.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A 2 by 2 white PNG, the image the clipboard is given.
fn png_2x2_white() -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, 2, 2);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&[255u8; 16])
            .map_err(|e| e.to_string())?;
    }
    Ok(buf)
}

/// Puts the 2 by 2 image on the in-memory clipboard. The answer is the app's own word: "ok", or why
/// it refused.
fn put_image(app: &mut App) -> String {
    let b64 = match png_2x2_white() {
        Ok(png) => base64(&png),
        Err(e) => return e,
    };
    match app.session.call(&Request::ClipSet {
        text: None,
        png_b64: Some(b64),
    }) {
        Ok(Reply::Ok) => "ok".into(),
        Ok(Reply::Err { message }) => message,
        Ok(other) => format!("{other:?}"),
        Err(e) => e,
    }
}

/// The question texts Sonara heard, in order: `REVIEW` for the review, else the question's text.
fn heard_texts(posts: &[Post]) -> Vec<String> {
    posts
        .iter()
        .map(|p| {
            if p.body.get("review").is_some() {
                "REVIEW".to_string()
            } else {
                p.body["text"].as_str().unwrap_or("").to_string()
            }
        })
        .collect()
}

/// The tab id Sonara was told, from the post's body (`host_tab`).
fn host_tab_of(post: &Post) -> String {
    post.body["host_tab"].as_str().unwrap_or("").to_string()
}

/// PT: run.mjs:3279-3377. Three restored Claude tabs: the first frame, the skeleton and the rings
/// while each conversation comes back, and no restored tab says it finished, asks or failed.
pub fn launch_skeleton(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, beta, gamma, profile, mut claude) = {
        let w = ctx.ensure_world();
        let gamma = w.root.join("gamma");
        let _ = std::fs::create_dir_all(&gamma);
        (
            w.alpha.clone(),
            w.beta.clone(),
            gamma,
            w.profile.clone(),
            FakeClaude::new(w),
        )
    };
    let folders = [alpha.clone(), beta.clone(), gamma.clone()];
    for cwd in &folders {
        claude.transcript(&cwd.to_string_lossy(), SESSION, "cli");
    }
    // The stand-in: its `claude` title, a banner, then (1.5 s on) the ready title and the READY line.
    claude.script(&[
        Step::Title("claude".into()),
        Step::Print("~\\agent-banner-path".into()),
        Step::Sleep(1500),
        Step::Title("\u{2733} Claude Code".into()),
        Step::Print(READY.into()),
    ]);
    std::fs::write(claude.bin().join("claude.cmd"), CLAUDE_CMD_NOISY)
        .map_err(|e| format!("write claude.cmd: {e}"))?;
    // The three tabs come back as Claude tabs (tabs.json is read at start-up).
    let saved = json!({
        "tabs": folders
            .iter()
            .map(|f| json!({ "cwd": f.to_string_lossy(), "agent": "claude" }))
            .collect::<Vec<_>>(),
        "active": 0
    });
    let _ = std::fs::create_dir_all(&profile);
    std::fs::write(profile.join("tabs.json"), saved.to_string())
        .map_err(|e| format!("write tabs.json: {e}"))?;

    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        env: claude.env(),
        ..Launch::default()
    })?;
    let outcome = launch_skeleton_checks(ctx, &mut app);
    app.close();
    outcome
}

fn launch_skeleton_checks(ctx: &mut Ctx, app: &mut App) -> Result<(), String> {
    // DROPPED: "the first frame already holds the three tabs, and no start screen". No probe reads the
    // first painted frame; the restored tab rows are checked from the first answer on.
    ctx.skip(
        "DROPPED: the first frame holds the three tabs (no probe reads the first painted frame)",
    );
    ctx.until("the three restored tabs", 15_000, &mut || {
        tab_count(app) == 3
    });

    let front = front_tab(app);
    // While it comes back: every tab shows the loading ring.
    let (rings, _) = look(app, &front);
    ctx.shot(app, "launch-skeleton");
    ctx.ok(
        rings == 3,
        format!("every tab shows the loading ring ({rings})"),
    );
    // DROPPED: "the tab in front wears the skeleton while its conversation comes back". The
    // skeleton's flag is an input property of the window that no probe reads back.
    ctx.skip(
        "DROPPED: the front tab wears the skeleton (the skeleton's flag is not read by any probe)",
    );
    // DROPPED: "each at the tab's end, where the Ring goes". The ring's place is laid out by the tab
    // itself; the tab rows do not report it.
    ctx.skip("DROPPED: each ring sits at the tab's end (a layout fact the tab rows do not report)");

    // It lifts once the agent appears: the front shell shows the agent's line and no ring is left.
    let settled = wait(30_000, &mut || {
        let (r, text) = look(app, &front);
        r == 0 && text.contains(READY)
    });
    let (r, text) = look(app, &front);
    ctx.ok(
        settled && r == 0 && text.contains(READY),
        format!(
            "the agent appears, and every ring goes (rings {r}, ready {})",
            text.contains(READY)
        ),
    );
    // DROPPED: "what the shell said before the agent was never shown uncovered". The shell's words
    // are under the skeleton, and the cover is a pixel fact no probe answers.
    ctx.skip("DROPPED: the shell's words are never shown uncovered (the cover is pixels; no probe answers)");
    // And it is not left in the terminal above the agent.
    let (_, text) = look(app, &front);
    ctx.ok(
        !text.contains(NOISE),
        "and it is not left in the terminal above the agent",
    );

    // A tab behind, clicked after its own agent was ready: its agent shows at once. (The skeleton
    // half of PT's check is the DROPPED skeleton check above.)
    bring_to_front(app, 2);
    let behind = tab_id_at(app, 2).unwrap_or_default();
    wait(5_000, &mut || front_tab(app) == behind);
    let behind_text = text_of(app, &behind);
    ctx.ok(
        behind_text.contains(READY),
        "a tab behind, opened after it was ready, shows its agent at once",
    );

    // A RESUMED CONVERSATION IS NOT NEW WORK: once the poll has answered for every tab, no tab says
    // it finished, asks or failed (rule 38; PT's `data-agent-state`, Flare's mark words).
    wait(15_000, &mut || polled(app));
    let marks: Vec<String> = app_state(app)["tabs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|t| t["mark"]["state"].as_str().unwrap_or("").to_string())
        .collect();
    ctx.ok(
        !marks.is_empty()
            && marks
                .iter()
                .all(|m| !matches!(m.as_str(), "finished" | "question" | "failed")),
        format!(
            "restored, no tab says it finished, asks or failed ({})",
            marks.join(", ")
        ),
    );
    Ok(())
}

/// PT: run.mjs:3756-3830. Sonara hears the question the widget shows, once per question, the review
/// last, always for the same tab.
pub fn sonara_question(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let home = alpha.join("sonara-home");
    let sonara = SonaraStandIn::start(&home, SONARA_KEY)?;
    let screens = question_screens()?;
    let probe_script = alpha.join("question-probe.ps1");
    write_script(&probe_script, &question_script(&screens))?;

    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha.clone()],
        env: sonara.env(),
        ..Launch::default()
    })?;
    let outcome = sonara_checks(ctx, &mut app, &sonara, &probe_script);
    app.close();
    outcome
}

fn sonara_checks(
    ctx: &mut Ctx,
    app: &mut App,
    sonara: &SonaraStandIn,
    probe_script: &Path,
) -> Result<(), String> {
    type_line(app, &run_script_line(probe_script, ""));
    let heard = wait(15_000, &mut || !sonara.posts().is_empty());
    ctx.ok(heard, "Sonara hears the question the widget shows");

    let posts = sonara.posts();
    let first = posts.first().cloned();
    let path = first.as_ref().map_or(String::new(), |p| p.path.clone());
    let auth = first.as_ref().is_some_and(|p| p.auth_ok);
    ctx.ok(
        path == QUESTION_PATH && auth,
        format!("posted to question_shown with runtime.json's key ({path})"),
    );
    let host = first.as_ref().map_or(String::new(), host_tab_of);
    ctx.ok(
        host.len() >= 16,
        format!("the tab is named by its shell's FLARE_TAB_ID ({host})"),
    );
    let texts = heard_texts(&posts);
    let first_text = texts.first().cloned().unwrap_or_default();
    ctx.ok(
        first_text == "Pick a colour?",
        format!("the first question ({first_text})"),
    );

    // A redraw of the same question (`x`) posts nothing: the settle time is held in 50 ms steps.
    let _ = app.session.type_text("x");
    quiet(ctx, 800);
    let after_redraw = sonara.posts().len();
    ctx.ok(
        after_redraw == 1,
        format!("a redraw of the same question says nothing ({after_redraw} posts)"),
    );

    // One move at a time: moves inside the settle time (150 ms) are told as the question landed on.
    let _ = app.session.type_text("n");
    let second = wait(5_000, &mut || sonara.posts().len() >= 2);
    ctx.ok(second, "moving to the next question is heard");
    let _ = app.session.type_text("n");
    let third = wait(5_000, &mut || sonara.posts().len() >= 3);
    ctx.ok(third, "the next move is heard");
    let _ = app.session.type_text("n");
    let review = wait(5_000, &mut || sonara.posts().len() >= 4);
    ctx.ok(review, "and the review after the last answer");

    let all = heard_texts(&sonara.posts());
    ctx.ok(
        all == ["Pick a colour?", "Pick a size?", "Pick a shape?", "REVIEW"],
        format!("in order, the review last ({})", all.join(" | ")),
    );
    let mut tabs: Vec<String> = sonara.posts().iter().map(host_tab_of).collect();
    tabs.sort();
    tabs.dedup();
    ctx.ok(tabs.len() == 1, "always the same tab id");
    let _ = app.session.type_text("q");
    Ok(())
}

/// PT: run.mjs:4016-4124. An image on Ctrl+V reaches a plain program as ^V and an armed Claude as
/// ESC v; Ctrl+D and Ctrl+C never reach an idle agent; Shift+Enter is Ctrl+J with an agent and CR
/// without, and Ctrl+J within a second once the title reads "Claude Code".
pub fn image_paste(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, mut claude) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), FakeClaude::new(w))
    };
    let plain = alpha.join("probe.ps1");
    write_script(&plain, &key_dump_script())?;
    // The stand-in for Claude: `claude` runs the key dump with the same tag argument.
    claude.script(&[]);
    write_script(&claude.bin().join("claude-steps.ps1"), &key_dump_script())?;
    std::fs::write(claude.bin().join("claude.cmd"), CLAUDE_CMD_FORWARDING)
        .map_err(|e| format!("write claude.cmd: {e}"))?;

    let mut app = ctx.launch(Launch {
        renderer: Some("software"),
        folders: vec![alpha.clone()],
        env: claude.env(),
        ..Launch::default()
    })?;
    let outcome = image_paste_checks(ctx, &mut app, &plain);
    app.close();
    outcome
}

fn image_paste_checks(ctx: &mut Ctx, app: &mut App, plain: &Path) -> Result<(), String> {
    ctx.until("one tab", 15_000, &mut || tab_count(app) == 1);
    wait(5_000, &mut || polled(app));
    mute_psreadline_paste(app);

    // A plain program: ^V.
    type_line(app, &run_script_line(plain, "P"));
    let plain_up = wait(15_000, &mut || front_text(app).contains("READY P"));
    ctx.ok(plain_up, "the plain probe is running");
    if !plain_up {
        return Err("the plain probe never started".into());
    }
    let clip = put_image(app);
    let _ = app.session.key("Ctrl+V");
    let pasted = wait(5_000, &mut || has_token(&front_text(app), "IN P 16"));
    ctx.ok(
        pasted,
        format!("an image on Ctrl+V sends ^V to a program that is no agent (clipboard: {clip})"),
    );
    let _ = app.session.type_text("q");

    // Claude, as the poll sees it: ESC v, and only that.
    type_line(app, "claude C");
    let claude_up = wait(15_000, &mut || front_text(app).contains("READY C"));
    ctx.ok(claude_up, "the stand-in for Claude is running");
    if !claude_up {
        return Err("the stand-in for Claude never started".into());
    }
    let present = wait(30_000, &mut || any_agent_present(app));
    ctx.ok(present, "and the poll calls it an agent");
    let clip = put_image(app);
    let _ = app.session.key("Ctrl+V");
    let alt_v = wait(5_000, &mut || has_token(&front_text(app), "IN C 1b 76"));
    ctx.ok(
        alt_v,
        format!("an image on Ctrl+V sends Alt+V (ESC v) to Claude (clipboard: {clip})"),
    );
    quiet(ctx, 500);
    let no_ctrl_v = !has_token(&front_text(app), "IN C 16");
    ctx.ok(no_ctrl_v, "and no ^V with it: one key, one paste");

    // KEEP AGENT SESSIONS OPEN: an idle agent hears neither Ctrl+D (exit) nor Ctrl+C (clear, exit).
    let _ = app.session.key("Ctrl+D");
    let _ = app.session.key("Ctrl+C");
    quiet(ctx, 600);
    let text = front_text(app);
    ctx.ok(
        !has_token(&text, "IN C 04") && !has_token(&text, "IN C 03"),
        "Ctrl+D and Ctrl+C never reach an idle agent (keep agent sessions open)",
    );
    let _ = app.session.key("Shift+Enter");
    let newline = wait(5_000, &mut || has_token(&front_text(app), "IN C 0a"));
    ctx.ok(
        newline,
        "Shift+Enter is Ctrl+J, Claude's own newline (#175)",
    );
    // xterm sent a CR on Enter's keypress too, so Claude submitted (2026-10-11).
    quiet(ctx, 500);
    let no_enter = !has_token(&front_text(app), "IN C 0d");
    ctx.ok(
        no_enter,
        "and no Enter with it: the newline does not submit",
    );
    let _ = app.session.type_text("q");
    let left = wait(45_000, &mut || !any_agent_present(app));
    ctx.ok(left, "the poll sees the agent leave");

    // #175: Claude's TITLE arms Shift+Enter before any poll could. The plain probe is no agent to the
    // poll, so only the title can have armed it.
    type_line(app, &run_script_line(plain, "T"));
    let titled_up = wait(15_000, &mut || front_text(app).contains("READY T"));
    ctx.ok(titled_up, "a plain probe again");
    let _ = app.session.key("Shift+Enter");
    let cr = wait(5_000, &mut || has_token(&front_text(app), "IN T 0d"));
    ctx.ok(cr, "with no agent, Shift+Enter is Enter (CR)");
    let _ = app.session.type_text("t");
    wait(5_000, &mut || front_text(app).contains("TITLED"));
    let _ = app.session.key("Shift+Enter");
    let armed = wait(1_000, &mut || has_token(&front_text(app), "IN T 0a"));
    ctx.ok(
        armed,
        "once it titles itself \"\u{2733} Claude Code\", Shift+Enter is Ctrl+J within 1 s",
    );
    quiet(ctx, 500);
    let one_cr = count_tokens(&front_text(app), "IN T 0d") == 1;
    ctx.ok(one_cr, "and no Enter with it");
    let _ = app.session.type_text("q");
    Ok(())
}

/// The check the port's helpers hold: the base64 of the RFC 4648 vectors, whole-word token counts,
/// the key-dump and question programs, and the fixture's four screens.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_token_counts_only_as_a_whole_word() {
        assert!(has_token("IN C 16\r\n", "IN C 16"));
        assert!(!has_token("IN C 160\r\n", "IN C 16"));
        assert!(has_token("x IN C 1b 76", "IN C 1b 76"));
        assert!(!has_token("IN C 0", "IN C 0d"));
    }

    #[test]
    fn the_cr_count_is_exact() {
        let text = "IN T 0d\r\nIN T 0d0\r\nIN T 0d\r\n";
        assert_eq!(count_tokens(text, "IN T 0d"), 2);
    }

    #[test]
    fn the_png_has_its_signature() {
        let png = png_2x2_white().expect("the image encodes");
        assert_eq!(&png[..4], &[137, 80, 78, 71]);
    }

    #[test]
    fn the_key_dump_prints_what_the_port_checks() {
        let body = key_dump_script();
        assert!(body.contains("TreatControlCAsInput"));
        assert!(body.contains("'IN '"));
        assert!(body.contains("TITLED"));
        assert!(body.contains("exit 0"));
    }

    #[test]
    fn the_question_program_quotes_its_screens() {
        let script = question_script(&["it's one".to_string(), "two".to_string()]);
        assert!(script.contains("'it''s one'"));
        assert!(script.contains("$screens.Count - 1"));
    }

    #[test]
    fn the_fixture_holds_the_four_screens_in_order() {
        let screens = question_screens().expect("the fixture parses");
        assert_eq!(screens.len(), 4);
        assert!(screens[0].contains("Pick a colour?"));
        assert!(screens[1].contains("Pick a size?"));
        assert!(screens[2].contains("Pick a shape?"));
        assert!(screens[3].contains("Review your answers"));
    }

    #[test]
    fn the_heard_texts_name_the_review() {
        let post = |body: Value| Post {
            method: "POST".into(),
            path: QUESTION_PATH.into(),
            auth_ok: true,
            status: 200,
            body,
            tab_id: None,
        };
        let texts = heard_texts(&[
            post(json!({ "host_tab": "t", "text": "Pick a colour?" })),
            post(json!({ "host_tab": "t", "review": true })),
        ]);
        assert_eq!(texts, ["Pick a colour?", "REVIEW"]);
    }

    #[test]
    fn the_run_line_names_the_script_and_its_argument() {
        let line = run_script_line(Path::new("C:\\w\\probe.ps1"), "P");
        assert!(line.ends_with("-File 'C:\\w\\probe.ps1' P"));
    }

    #[test]
    fn the_stand_in_claude_forwards_its_arguments() {
        assert!(CLAUDE_CMD_FORWARDING.contains("%*"));
        assert!(CLAUDE_CMD_NOISY.contains("NOISE-BEFORE"));
    }
}
