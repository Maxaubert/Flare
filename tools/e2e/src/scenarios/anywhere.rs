//! The dictate-from-any-app scenarios (M8 Task 31): `dictateAnywhere` (PT run.mjs 3172, 9 checks) and
//! `dictateTwoCopies` (3261, 10 checks).
//!
//! The suite never installs the real keyboard hook: helper lines arrive through `Request::DictLine`, and what was
//! heard is handed over with `DictRouted` (the window's answer) or `DictRoute` (a recording that began with some
//! window in front). A stand-in Claude (a copy of PowerShell named `claude.exe`, so Windows' process check passes)
//! takes the words and runs them, which proves they were typed AND entered. The microphone is the fixture clip, so
//! the real one is never opened (the key in part 3 starts a recording).
//!
//! Every PT check keeps its place and message. Flare answers the widget through `dict.widget`; PT's card text is
//! the widget's title, words, rows and headers joined (`widget_text`).
//!
//! `dictateAnywhere` part 2 (PT: an ACTIVE orchestrator, no list) is REPLACED: Flare has no orchestrator (DIFF-16),
//! so the three checks become "lists again", "nothing is sent without a pick" and "a pick sends it".

use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flare_e2e_proto::{dict_probes, Request, RouteResult};
use serde_json::{json, Value};

use crate::ctx::{App, Ctx, Launch};
use crate::fixtures;
use crate::standins::hooklines::hook_line;
use crate::world::{prompt_showing, prompt_timeout, rows_text, wait_for_prompt, Shell, World};

/// The clip the microphone plays: the real one is never opened (part 3 starts a recording).
const CLIP_FIXTURE: &str = "jfk.wav";
/// The stand-in Claude's source: the Windows PowerShell, copied under the name Windows' process check wants.
const POWERSHELL: &str = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";

fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// Holds the scenario still for `ms` (PT's `sleep`), through the one wait.
fn pause(ctx: &mut Ctx, ms: u64) {
    let start = Instant::now();
    ctx.until("a pause to pass", ms + 5_000, &mut || {
        start.elapsed() >= Duration::from_millis(ms)
    });
}

/// PT's `card.textContent`: the widget's head line, words, rows (number, label, folder, chip) and headers, joined.
pub fn widget_text_of(w: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    for key in ["title", "text"] {
        if let Some(s) = w[key].as_str() {
            parts.push(s.to_string());
        }
    }
    for h in w["headers"].as_array().into_iter().flatten() {
        if let Some(s) = h.as_str() {
            parts.push(s.to_string());
        }
    }
    for r in w["rows"].as_array().into_iter().flatten() {
        if let Some(n) = r["n"].as_i64() {
            parts.push(n.to_string());
        }
        for key in ["label", "folder", "chip", "reason"] {
            if let Some(s) = r[key].as_str() {
                parts.push(s.to_string());
            }
        }
    }
    parts.join(" ")
}

/// The widget's text now (empty while it does not exist).
fn widget_text(app: &mut App) -> String {
    widget_text_of(&probe(app, dict_probes::WIDGET, Value::Null))
}

/// The widget's group headers, in order.
pub fn headers_of(w: &Value) -> Vec<String> {
    w["headers"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|h| h.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// PT's `/^PASTEDB\s*$/m`: some line of `text` is the word alone (the shell ran it and printed it).
pub fn has_line_alone(text: &str, word: &str) -> bool {
    text.lines().any(|l| l.trim() == word)
}

/// 64 hex digits from the process's own random hashing state: the scenario's link key (no crate needed).
pub fn random_key_hex() -> String {
    let mut out = String::new();
    for _ in 0..4 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(std::process::id() as u64);
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// Writes the profile's settings before launch: dictation on, the sounds and the media pause off (a test never
/// plays a cue or pauses the owner's media). The hotkey stays the default, Ctrl+Win.
fn seed(profile: &Path) -> Result<(), String> {
    fs::create_dir_all(profile).map_err(|e| e.to_string())?;
    let text = json!({
        "flare.dictation.enabled": "1",
        "flare.dictation.sounds": "0",
        "flare.dictation.pauseMedia": "0",
    })
    .to_string();
    fs::write(profile.join("settings.json"), text).map_err(|e| e.to_string())
}

/// The environment of a dictation copy: the clip as the microphone, its own models folder, no GPU, and
/// any link seam the scenario passes.
fn dict_env(profile: &Path, clip: &Path, extra: &[(String, String)]) -> Vec<(String, String)> {
    let mut env = vec![
        (
            concat!("FLARE_E2E", "_MIC").to_string(),
            clip.to_string_lossy().into_owned(),
        ),
        (
            concat!("FLARE_DICTATION", "_ROOT").to_string(),
            profile.join("dictation").to_string_lossy().into_owned(),
        ),
        (concat!("FLARE_E2E", "_NVIDIA").to_string(), "0".to_string()),
    ];
    env.extend(extra.iter().cloned());
    env
}

/// The first shell tab's id.
fn shell_tab(app: &mut App) -> String {
    app.session
        .state()
        .ok()
        .and_then(|s| {
            s.tabs
                .iter()
                .find(|t| t.kind == "shell")
                .map(|t| t.id.clone())
        })
        .unwrap_or_default()
}

/// Clicks the middle of the terminal grid, so the keys go to the shell.
fn click_grid(app: &mut App) {
    let r = probe(app, "ui.rect", json!({ "name": "grid" }));
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

/// PT `typeLine`: waits for the prompt in the tab, then types the line and Enter.
fn type_line(app: &mut App, tab: &str, cmd: &str) {
    wait_for_prompt(
        app,
        tab,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    );
    let _ = app.session.type_text(cmd);
    let _ = app.session.key("Enter");
}

/// PT `hookSay('done', ';session=<sid>')`: a PowerShell line that writes the hook line of a finished turn.
fn say_done(session: &str) -> String {
    let raw = hook_line("done", &json!({ "session": session }));
    let body = raw
        .trim_start_matches('\u{1b}')
        .trim_end_matches('\u{7}')
        .replace('\'', "''");
    format!("[Console]::Write([char]27 + '{body}' + [char]7)")
}

/// The stand-in Claude: a copy of PowerShell named `claude.exe` runs in the tab (Windows' process tree then says
/// Claude), and inside it the hook line of a finished turn names a Claude session. **[seam]** PT prints the hook
/// line first and starts the stand-in after; Flare takes a tab's session back at its next shell prompt (the agent
/// has left), so the line is printed from inside the stand-in, where no prompt of the app's shell follows it, as a
/// real Claude's hook does. Leaves the shell inside the stand-in.
fn start_stand_in(
    ctx: &mut Ctx,
    app: &mut App,
    profile: &Path,
    session: &str,
) -> Result<(), String> {
    let tab = shell_tab(app);
    ctx.until(
        "the PowerShell prompt",
        prompt_timeout(Shell::PowerShell).as_millis() as u64,
        &mut || prompt_showing(&rows_text(app, &tab), Shell::PowerShell),
    );
    // PT ``polled``: the process poll has said its first word on the shell (it would clear what comes before it).
    crate::world::wait_for_agent_verdict(app, &tab, Duration::from_secs(30));
    click_grid(app);
    let dir = profile.join("stand-in");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let fake = dir.join("claude.exe");
    fs::copy(POWERSHELL, &fake).map_err(|e| e.to_string())?;
    type_line(
        app,
        &tab,
        &format!("& '{}' -NoLogo -NoProfile", fake.display()),
    );
    pause(ctx, 2_500);
    type_line(app, &tab, &say_done(session));
    pause(ctx, 1_000);
    Ok(())
}
/// One helper line, as if the hook had printed it.
fn line(app: &mut App, l: &str) {
    if let Err(e) = app.session.call(&Request::DictLine { line: l.into() }) {
        println!("  note  the helper line {l:?} was refused: {e}");
    }
}

/// What the window hands main as heard: the text.
fn routed(app: &mut App, text: &str) {
    let r = app.session.call(&Request::DictRouted {
        result: RouteResult::Text { text: text.into() },
    });
    if let Err(e) = r {
        println!("  note  the heard text was refused: {e}");
    }
}

/// A recording that began with the window of process `front` in front (or none) heard `text`.
fn route(app: &mut App, text: &str, front: Option<u32>) {
    let r = app.session.call(&Request::DictRoute {
        text: text.into(),
        front,
    });
    if let Err(e) = r {
        println!("  note  the route was refused: {e}");
    }
}

fn world_paths(ctx: &mut Ctx) -> (PathBuf, PathBuf) {
    let w = ctx.ensure_world();
    (w.profile.clone(), w.alpha.clone())
}

/// PT `dictateAnywhere` (9 assertions): Task 31. Part 1 lists the Claude sessions and a number sends; part 2
/// (REPLACED, DIFF-16) lists again and sends nothing without a pick; part 3 opens the widget from the key with no
/// Flare window in front.
pub fn dictate_anywhere(ctx: &mut Ctx) -> Result<(), String> {
    let clip = fixtures::require(CLIP_FIXTURE)?;
    let (profile, alpha) = world_paths(ctx);
    seed(&profile)?;
    let mut app = ctx.launch(Launch {
        folders: vec![alpha],
        env: dict_env(&profile, &clip, &[]),
        ..Launch::default()
    })?;
    let sid = "f1f2f3f4-a5a6-4b70-8c9d-0e1f2a3b4c5d";
    let tab = shell_tab(&mut app);
    start_stand_in(ctx, &mut app, &profile, sid)?;

    // 1. No orchestrator: the list, then "1".
    // PT: run.mjs:3199
    routed(&mut app, "Write-Output ('DICT-' + 'OK')");
    let listed = ctx.until("the widget to list the sessions", 10_000, &mut || {
        widget_text(&mut app).contains("Send to")
    });
    ctx.ok(listed, "with no orchestrator the widget lists the sessions");
    // PT: run.mjs:3201
    let w1 = widget_text(&mut app);
    ctx.ok(
        w1.contains('1') && w1.contains("alpha"),
        format!(
            "numbered, by tab ({})",
            w1.chars().take(120).collect::<String>()
        ),
    );
    // PT: run.mjs:3206
    let w = probe(&mut app, dict_probes::WIDGET, Value::Null);
    ctx.ok(
        w["focusable"] == json!(false) && w["topmost"] == json!(true),
        "the widget never takes focus and stays on top",
    );
    ctx.shot(&mut app, "dictation-widget-pick");
    line(&mut app, "p 1");
    // PT: run.mjs:3210
    let sent = ctx.until("the widget to say sent", 15_000, &mut || {
        widget_text(&mut app).contains("Sent to alpha")
    });
    ctx.ok(sent, "the number sends it, and the widget says sent");
    ctx.shot(&mut app, "dictation-widget-sent");
    // PT: run.mjs:3212
    let typed = ctx.until("DICT-OK in the session", 8_000, &mut || {
        rows_text(&mut app, &tab).contains("DICT-OK")
    });
    ctx.ok(typed, "and it was typed and entered in that session");

    // 2. REPLACED: DIFF-16, Flare has no orchestrator. A second heard text lists again, sends nothing
    // without a pick, and a pick sends it.
    pause(ctx, 3_000); // let "Sent to alpha" leave
    routed(&mut app, "Write-Output ('LEAD-' + 'OK')");
    // PT: run.mjs:3223 (REPLACED: DIFF-16, was "it is sent straight there")
    let again = ctx.until("the widget to list again", 10_000, &mut || {
        widget_text(&mut app).contains("Send to")
    });
    ctx.ok(again, "a second heard text lists the sessions again");
    // PT: run.mjs:3224 (REPLACED: DIFF-16, was "with no list")
    pause(ctx, 3_000);
    ctx.ok(
        !rows_text(&mut app, &tab).contains("LEAD-OK"),
        "nothing is sent without a pick",
    );
    line(&mut app, "p 1");
    // PT: run.mjs:3225 (REPLACED: DIFF-16, was "and it arrived")
    let arrived = ctx.until("LEAD-OK in the session", 8_000, &mut || {
        rows_text(&mut app, &tab).contains("LEAD-OK")
    });
    ctx.ok(arrived, "a pick sends it");

    // 3. The key, with the app not in front (the suite's window is parked and unfocused): Ctrl then Win,
    // held past the solo wait.
    pause(ctx, 3_000);
    line(&mut app, "d A2");
    line(&mut app, "d 5B");
    pause(ctx, 500);
    // PT: run.mjs:3239
    let keyed = widget_text(&mut app);
    ctx.ok(
        keyed.contains("Listening")
            || keyed.contains("speech model")
            || keyed.contains("Speak now"),
        format!(
            "Ctrl+Win from another app opens the widget ({})",
            keyed.chars().take(80).collect::<String>()
        ),
    );
    line(&mut app, "u 5B");
    line(&mut app, "u A2");
    let _ = app.close();
    Ok(())
}

/// PT `dictateTwoCopies` (10 assertions): Task 31. Two copies on a pipe of the scenario's own; A is made the
/// service through the seam. The list holds both copies' Claude under a header each; a pick is typed and entered by
/// the other copy; with B's window in front the words are pasted at its cursor, no Enter; the service closes and B
/// takes over.
pub fn dictate_two_copies(ctx: &mut Ctx) -> Result<(), String> {
    let clip = fixtures::require(CLIP_FIXTURE)?;
    let wa = World::new();
    let wb = World::new();
    seed(&wa.profile)?;
    seed(&wb.profile)?;
    // The scenario's own link: both copies prove they hold the key.
    let pipe = format!(
        r"\\.\pipe\flare-e2e-dict-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis())
    );
    let link = vec![
        (concat!("FLARE_E2E", "_DICT_PIPE").to_string(), pipe),
        (
            concat!("FLARE_E2E", "_DICT_KEY").to_string(),
            random_key_hex(),
        ),
    ];
    let mut a = ctx.launch_in(
        &wa,
        Launch {
            folders: vec![wa.alpha.clone()],
            env: dict_env(&wa.profile, &clip, &link),
            ..Launch::default()
        },
    )?;
    let mut b = ctx.launch_in(
        &wb,
        Launch {
            folders: vec![wb.beta.clone()],
            env: dict_env(&wb.profile, &clip, &link),
            ..Launch::default()
        },
    )?;
    let tab_b = shell_tab(&mut b);

    // B's stand-in Claude, then A's, so the list holds both apps.
    start_stand_in(
        ctx,
        &mut b,
        &wb.profile,
        "b1b2b3b4-a5a6-4b70-8c9d-0e1f2a3b4c5d",
    )?;
    start_stand_in(
        ctx,
        &mut a,
        &wa.profile,
        "c1c2c3c4-a5a6-4b70-8c9d-0e1f2a3b4c5d",
    )?;

    // A takes the key: the service. B finds it on the pipe.
    line(&mut a, "ready");
    pause(ctx, 1_800);

    // 1. A's list holds both copies' Claude, under a header each; "2" is B's.
    route(&mut a, "Write-Output ('TWO-' + 'OK')", None);
    // PT: run.mjs:3308
    let listed = ctx.until("the service to list the sessions", 10_000, &mut || {
        widget_text(&mut a).contains("Send to")
    });
    ctx.ok(listed, "the service lists the sessions");
    let text = widget_text(&mut a);
    // PT: run.mjs:3312
    ctx.ok(
        text.contains("alpha") && text.contains("beta"),
        format!(
            "both copies' Claude are in the list ({})",
            text.chars().take(160).collect::<String>()
        ),
    );
    // PT: run.mjs:3315 (a header per copy, the service's own first; D8 names)
    let (first, second) = if ctx.packaged {
        ("Flare", "Flare (2)")
    } else {
        ("Flare (Dev)", "Flare (Dev) (2)")
    };
    let headers = headers_of(&probe(&mut a, dict_probes::WIDGET, Value::Null));
    ctx.ok(
        headers == [first, second],
        format!("under a header per copy ({headers:?})"),
    );
    ctx.shot(&mut a, "dictation-widget-groups");
    line(&mut a, "p 2");
    // PT: run.mjs:3318
    let sent = ctx.until("the widget to say sent to beta", 15_000, &mut || {
        widget_text(&mut a).contains("Sent to beta")
    });
    ctx.ok(sent, "the pick is sent to the other copy");
    // PT: run.mjs:3319
    let entered = ctx.until("TWO-OK in B", 8_000, &mut || {
        rows_text(&mut b, &tab_b).contains("TWO-OK")
    });
    ctx.ok(entered, "and that copy typed and entered it");

    // 2. B's window in front at the press: pasted at its cursor, no Enter.
    let pid_b = probe(&mut b, "app.pid", Value::Null)
        .as_u64()
        .map(|p| p as u32);
    route(&mut a, "echo PASTEDB", pid_b);
    // PT: run.mjs:3324
    let pasted = ctx.until("echo PASTEDB at B's cursor", 8_000, &mut || {
        rows_text(&mut b, &tab_b).contains("echo PASTEDB")
    });
    ctx.ok(
        pasted,
        "with that copy in front the words are typed at its cursor",
    );
    pause(ctx, 800);
    // PT: run.mjs:3327
    ctx.ok(
        !has_line_alone(&rows_text(&mut b, &tab_b), "PASTEDB"),
        "and not entered",
    );
    // PT: run.mjs:3328
    ctx.ok(
        widget_text(&mut a).contains("Typed into the tab in front"),
        "the widget says where they went",
    );
    let _ = b.session.key("Ctrl+C");

    // 3. The service closes: B takes over and lists its own tab.
    let _ = a.close();
    pause(ctx, 1_500);
    line(&mut b, "ready");
    route(&mut b, "Write-Output hi", None);
    // PT: run.mjs:3337
    let took = ctx.until("B to list its own tab", 10_000, &mut || {
        widget_text(&mut b).contains("Send to")
    });
    ctx.ok(took, "the other copy takes over the dictation");
    let own = widget_text(&mut b);
    // PT: run.mjs:3339
    ctx.ok(
        own.contains("beta") && !own.contains(first),
        format!(
            "with one copy left, no headers ({})",
            own.chars().take(120).collect::<String>()
        ),
    );
    line(&mut b, "x");
    let _ = b.close();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_widget_text_joins_the_head_the_words_the_rows_and_the_headers() {
        let w = json!({
            "title": "Send to",
            "text": "hello",
            "headers": ["Flare (Dev)"],
            "rows": [{"n": 1, "label": "alpha", "folder": "dir", "chip": "", "reason": ""}],
        });
        let t = widget_text_of(&w);
        assert!(t.contains("Send to") && t.contains("hello") && t.contains("Flare (Dev)"));
        assert!(t.contains('1') && t.contains("alpha"));
        assert_eq!(widget_text_of(&Value::Null), "");
    }

    #[test]
    fn the_headers_are_read_in_order_and_a_missing_list_is_empty() {
        assert_eq!(headers_of(&json!({"headers": ["a", "b"]})), ["a", "b"]);
        assert!(headers_of(&Value::Null).is_empty());
    }

    #[test]
    fn a_word_alone_on_a_line_is_the_shell_having_run_it() {
        assert!(has_line_alone("echo x\nPASTEDB\nPS>", "PASTEDB"));
        assert!(has_line_alone("PASTEDB   ", "PASTEDB"));
        assert!(!has_line_alone("PS C:\\> echo PASTEDB", "PASTEDB"));
    }

    #[test]
    fn the_link_key_is_32_bytes_of_hex_and_never_the_same_twice() {
        let a = random_key_hex();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, random_key_hex());
    }

    #[test]
    fn the_done_line_names_the_session_in_the_frame_the_app_reads() {
        let l = say_done("f1f2f3f4-a5a6-4b70-8c9d-0e1f2a3b4c5d");
        assert!(l.contains("state=done;session=f1f2f3f4-a5a6-4b70-8c9d-0e1f2a3b4c5d"));
        assert!(l.starts_with("[Console]::Write([char]27 + ']777;flare-agent;"));
    }
}
