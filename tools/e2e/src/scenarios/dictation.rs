//! The dictation scenarios (M8 Tasks 29 and 30): `dictation`, `dictationPage`, `dictationParakeet`.
//! Task 29 ports `dictation` (PT run.mjs 8084, 21 checks) and `dictationParakeet` (8352, 11 checks).
//! `dictationPage` stays a stub until Task 30.
//!
//! Every PT check keeps its place and its message. Flare answers the state through the `dict.*` probes,
//! the Settings rows through `settings.row`, and the terminal through `grid.text`; PT's DOM reads become
//! those probes. The pill is read from `dict.pill`, which answers `mounted: false` while the pill is not
//! in the window's tree, so a pill check fails until the pill is mounted (Task 23's wiring).
//!
//! The profile's `settings.json` is seeded before launch with the Right Alt hotkey (as PT's scenarios
//! set it), the sounds off, and media pause off, so no real media is ever paused by a test.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flare_e2e_proto::Request;
use serde_json::{json, Value};

use super::driver;
use crate::ctx::{App, Ctx, Launch};
use crate::dictproc;
use crate::fixtures;
use crate::world::{prompt_showing, prompt_timeout, rows_text, Shell};

/// The hotkey the scenarios set: Right Alt alone (PT's scenarios held it before Ctrl+Win became the default).
const RIGHT_ALT: &str =
    r#"{"code":"AltRight","ctrl":false,"alt":false,"shift":false,"meta":false}"#;
/// The sentence the clip says (whisper.cpp's JFK sample), as PT matches it.
const SENTENCE: &str = "ask not what your country";
/// The cached fixtures these scenarios stage (Task 28).
const TINY_FIXTURE: &str = "ggml-tiny.bin";
const PARAKEET_FIXTURE: &str = "ggml-parakeet-tdt-0.6b-v3-q4_0.bin";
const CLIP_FIXTURE: &str = "jfk.wav";

/// The speech engine folder the scenarios read: `vendor/whisper` of this checkout (PT's `vendor/whisper`),
/// or the installed app's `whisper` folder when the packaged build runs. The path is lexical, with no `..`,
/// so the process paths the list returns compare equal to it.
fn engine_dir(packaged: bool) -> PathBuf {
    if packaged {
        let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
        return PathBuf::from(local)
            .join("Flare")
            .join("current")
            .join("whisper");
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest.parent().and_then(Path::parent).unwrap_or(manifest);
    repo.join("vendor").join("whisper")
}

/// PT's `/PS [^>]*>/g` count over a terminal's text: each `PS ` that a `>` closes.
pub fn count_prompts(text: &str) -> usize {
    let mut n = 0;
    let mut rest = text;
    while let Some(at) = rest.find("PS ") {
        let after = &rest[at + 3..];
        match after.find('>') {
            Some(gt) => {
                n += 1;
                rest = &after[gt + 1..];
            }
            None => break,
        }
    }
    n
}

/// True when the terminal text, with its white space collapsed, says the clip's sentence (PT's `/ask not what your country/i`).
pub fn sentence_heard(text: &str) -> bool {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .contains(SENTENCE)
}

/// Counts a `dictproc` answer, a failed list counting as "many" so no `== 0` or `== 1` check can pass on it.
fn ours(r: Result<usize, String>) -> usize {
    r.unwrap_or(usize::MAX)
}

/// `(x, y, w, h)` of a rectangle value, or `None` when a side is missing.
fn rect4(v: &Value) -> Option<(f64, f64, f64, f64)> {
    Some((
        v["x"].as_f64()?,
        v["y"].as_f64()?,
        v["w"].as_f64()?,
        v["h"].as_f64()?,
    ))
}

fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// `dict.view`: what the controller holds (active, listening, transcribing, session, live text, level).
fn view(app: &mut App) -> Value {
    probe(app, "dict.view", Value::Null)
}

fn flag(v: &Value, key: &str) -> bool {
    v[key] == json!(true)
}

fn active(app: &mut App) -> bool {
    flag(&view(app), "active")
}

/// The pill is in the tree and shows Listening (`dict.pill`: mounted, face 1).
fn pill_listening(app: &mut App) -> bool {
    let p = probe(app, "dict.pill", Value::Null);
    p["mounted"] == json!(true) && p["face"] == json!(1)
}

fn level(app: &mut App) -> f64 {
    view(app)["level"].as_f64().unwrap_or(0.0)
}

fn live_text(app: &mut App) -> String {
    view(app)["live_text"].as_str().unwrap_or("").to_string()
}

/// The tab ids that wear the dictation mark (`dict.mark`).
fn mark_tabs(app: &mut App) -> Vec<String> {
    probe(app, "dict.mark", Value::Null)["tabs"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The top of the terminal's grid in css px (`ui.rect`), the thing "the pill does not move the terminal" compares.
fn grid_top(app: &mut App) -> Option<f64> {
    rect4(&probe(app, "ui.rect", json!({ "name": "grid" }))).map(|r| r.1)
}

/// Clicks the middle of the terminal grid, so the keys go to the shell.
fn click_grid(app: &mut App) {
    if let Some((x, y, w, h)) = rect4(&probe(app, "ui.rect", json!({ "name": "grid" }))) {
        let _ = app
            .session
            .click((x + w / 2.0) as f32, (y + h / 2.0) as f32);
    }
}

/// The first shell tab's id (the tab the scenario types into).
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

/// Holds or releases one physical key by its code (`AltRight`, `ControlLeft`).
fn key(app: &mut App, chord: &str, down: bool) {
    // A key the app refuses is said on stdout (not a check: the checks are PT's), so it can never
    // pass a scenario silently.
    if let Err(e) = app.session.call(&Request::Key {
        chord: chord.into(),
        down,
    }) {
        println!("  note  the key {chord} (down {down}) was not sent: {e}");
    }
}

/// Presses one physical key, down and up.
fn press(app: &mut App, chord: &str) {
    let _ = app.session.key(chord);
}

/// Holds the scenario still for `ms` (PT's `sleep`), through the one wait.
fn pause(ctx: &mut Ctx, ms: u64) {
    let start = Instant::now();
    ctx.until("a pause to pass", ms + 5_000, &mut || {
        start.elapsed() >= Duration::from_millis(ms)
    });
}

/// The switch of Settings row `id` (`settings.row`'s `on`), `None` when the row is not drawn.
fn switch_on(app: &mut App, id: &str) -> Option<bool> {
    probe(app, "settings.row", json!({ "id": id }))["on"].as_bool()
}

/// Clicks the switch of row `id`, at the right end of its row inside the 16 px padding.
fn click_switch(app: &mut App, id: &str) -> bool {
    let r = probe(app, "settings.row", json!({ "id": id }));
    let Some((x, y, w, h)) = rect4(&r["rect"]) else {
        return false;
    };
    app.session
        .click((x + w - 16.0 - 18.0) as f32, (y + h / 2.0) as f32)
        .is_ok()
}

/// PT's `setPref` for a switch: the row is brought into view and its switch set to `want`.
fn set_switch(ctx: &mut Ctx, app: &mut App, id: &str, want: bool) -> bool {
    if !driver::goto_row(ctx, app, id) {
        return false;
    }
    if switch_on(app, id) == Some(want) {
        return true;
    }
    click_switch(app, id);
    ctx.until(
        &format!("the switch {id} to read {want}"),
        4_000,
        &mut || switch_on(app, id) == Some(want),
    )
}

/// Brings the shell tab in front and waits until Settings is gone.
fn back_to_shell(ctx: &mut Ctx, app: &mut App) {
    let rect = app.session.state().ok().and_then(|s| {
        s.tabs
            .iter()
            .find(|t| t.kind == "shell")
            .map(|t| (t.rect.x, t.rect.y, t.rect.w, t.rect.h))
    });
    if let Some((x, y, w, h)) = rect {
        let _ = app.session.click(x + w / 2.0, y + h / 2.0);
    }
    ctx.until("the shell tab in front", 6_000, &mut || {
        !driver::settings_front(app)
    });
}

/// Writes settings into a closed profile, so the launch starts from them (a JSON object of strings).
fn seed(profile: &Path, pairs: &[(&str, &str)]) -> Result<(), String> {
    fs::create_dir_all(profile).map_err(|e| e.to_string())?;
    let map: serde_json::Map<String, Value> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), json!(v)))
        .collect();
    let text = serde_json::to_string(&map).map_err(|e| e.to_string())?;
    fs::write(profile.join("settings.json"), text).map_err(|e| e.to_string())
}

/// Copies a cached model into the scenario's models folder under the name the store reads (`models/<id>.bin`).
fn stage_model(root: &Path, model: &Path, file: &str) -> Result<(), String> {
    let dir = root.join("models");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::copy(model, dir.join(file))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Starts the app on the scenario's profile: the clip as the microphone, the models folder, the GPU probe
/// answered "no", and the engine folder (unless the packaged build finds its own).
fn start(
    ctx: &mut Ctx,
    dict_root: &Path,
    clip: &Path,
    engine: &Path,
    alpha: &Path,
) -> Result<App, String> {
    let mut env = vec![
        (
            concat!("FLARE_E2E", "_MIC").to_string(),
            clip.to_string_lossy().into_owned(),
        ),
        (
            concat!("FLARE_DICTATION", "_ROOT").to_string(),
            dict_root.to_string_lossy().into_owned(),
        ),
        (concat!("FLARE_E2E", "_NVIDIA").to_string(), "0".to_string()),
    ];
    if !ctx.packaged {
        env.push((
            concat!("FLARE_WHISPER", "_DIR").to_string(),
            engine.to_string_lossy().into_owned(),
        ));
    }
    ctx.launch(Launch {
        folders: vec![alpha.to_path_buf()],
        env,
        ..Launch::default()
    })
}

/// The world's profile and first folder, cloned out of the context so the scenario can keep checking.
fn world_paths(ctx: &mut Ctx) -> (PathBuf, PathBuf) {
    let w = ctx.ensure_world();
    (w.profile.clone(), w.alpha.clone())
}

/// PT `dictation` (21 assertions): Task 29. Off means off; then Settings switches it on, the engine is
/// resident before any press, the AltGr chord types and does not dictate, holding Right Alt listens, the
/// clip arrives on the prompt line with no Enter, Escape cancels, and switching off stops the engine.
pub fn dictation(ctx: &mut Ctx) -> Result<(), String> {
    let engine = engine_dir(ctx.packaged);
    let model = fixtures::require(TINY_FIXTURE)?;
    let clip = fixtures::require(CLIP_FIXTURE)?;
    let (profile, alpha) = world_paths(ctx);
    let dict_root = profile.join("dictation");
    stage_model(&dict_root, &model, "tiny.bin")?;

    ctx.ok(
        engine.join("whisper-server.exe").is_file(),
        "the speech engine is in vendor/whisper",
    );
    ctx.ok(
        engine.join("vcomp140.dll").is_file() && engine.join("LICENSE-whisper.cpp.txt").is_file(),
        "with the C++ runtime and the MIT notice beside it",
    );
    // Right Alt, the tiny model, no sounds, no media pause (a test never pauses the owner's media).
    seed(
        &profile,
        &[
            ("flare.dictation.hotkey", RIGHT_ALT),
            ("flare.dictation.model", "tiny"),
            ("flare.dictation.sounds", "0"),
            ("flare.dictation.pauseMedia", "0"),
        ],
    )?;
    let mut app = start(ctx, &dict_root, &clip, &engine, &alpha)?;
    let tab = shell_tab(&mut app);
    ctx.until(
        "the PowerShell prompt",
        prompt_timeout(Shell::PowerShell).as_millis() as u64,
        &mut || prompt_showing(&rows_text(&mut app, &tab), Shell::PowerShell),
    );

    // OFF MEANS OFF: the key does nothing, and nothing is running.
    click_grid(&mut app);
    key(&mut app, "AltRight", true);
    pause(ctx, 700);
    let quiet = !active(&mut app) && !flag(&view(&mut app), "listening");
    key(&mut app, "AltRight", false);
    ctx.ok(quiet, "with dictation off, the key does nothing");
    ctx.ok(
        ours(dictproc::speech_servers(&engine)) == 0,
        "and no speech server exists",
    );

    // On, from the Settings page, with the e2e's model as the active one.
    let switched = set_switch(ctx, &mut app, "dictation-enabled", true);
    ctx.ok(switched, "the Dictation page switches it on");
    // WARM BEFORE ANYBODY SPEAKS (2026-09-28): switched on, the engine is started a moment later.
    let warm = ctx.until("the speech engine to start", 15_000, &mut || {
        ours(dictproc::speech_servers(&engine)) == 1
    });
    ctx.ok(
        warm,
        "switched on, the speech engine is started before any press",
    );
    back_to_shell(ctx, &mut app);
    click_grid(&mut app);
    let top_before = grid_top(&mut app);

    // AltGr TYPING IS NOT DICTATION: Ctrl+RightAlt with a key, as a Norwegian keyboard sends for "@",
    // must never start a dictation.
    key(&mut app, "ControlLeft", true);
    key(&mut app, "AltRight", true);
    pause(ctx, 60);
    press(&mut app, "Digit2");
    pause(ctx, 400);
    let altgr_quiet = !active(&mut app);
    ctx.ok(altgr_quiet, "AltGr typing does not start a dictation");
    key(&mut app, "AltRight", false);
    key(&mut app, "ControlLeft", false);
    press(&mut app, "Escape");

    // Hold, speak (the fake microphone does), release.
    key(&mut app, "AltRight", true);
    pause(ctx, 300);
    let listening = ctx.until("the pill to show Listening", 8_000, &mut || {
        pill_listening(&mut app)
    });
    ctx.ok(listening, "holding Right Alt opens the pill: Listening");
    let marked = mark_tabs(&mut app).contains(&tab);
    ctx.ok(marked, "and the tab wears the mic mark");
    let top_now = grid_top(&mut app);
    ctx.ok(
        top_now.is_some() && top_now == top_before,
        "the pill does not move the terminal",
    );
    let moved = ctx.until("the level meter to move", 8_000, &mut || {
        level(&mut app) > 0.2
    });
    ctx.ok(
        moved,
        "the level meter moves while the clip plays: a live mic is visible at once",
    );
    let mut live = String::new();
    let got_live = ctx.until("live text to appear", 15_000, &mut || {
        live = live_text(&mut app);
        !live.trim().is_empty()
    });
    ctx.ok(
        got_live,
        format!(
            "live text appears while still listening (\"{}\")",
            live.trim()
        ),
    );
    pause(ctx, 4_000);
    ctx.shot(&mut app, "listening");
    pause(ctx, 5_000); // let the whole sentence play
    let before = rows_text(&mut app, &tab);
    key(&mut app, "AltRight", false);
    let transcribing = ctx.until("the Transcribing state", 4_000, &mut || {
        flag(&view(&mut app), "transcribing")
    });
    ctx.ok(transcribing, "releasing says Transcribing");
    let heard = ctx.until(
        "the spoken sentence on the prompt line",
        30_000,
        &mut || sentence_heard(&rows_text(&mut app, &tab)),
    );
    ctx.ok(heard, "the spoken sentence arrives on the prompt line");
    let gone = ctx.until("the pill to go", 5_000, &mut || !active(&mut app));
    ctx.ok(gone, "and the pill goes away");
    let after = rows_text(&mut app, &tab);
    ctx.ok(
        count_prompts(&after) == count_prompts(&before),
        "NO ENTER was sent: there is no new prompt, the text is still being edited",
    );
    ctx.ok(
        ours(dictproc::speech_servers(&engine)) == 1,
        "one speech server is resident while dictation is on",
    );

    // Escape cancels and pastes nothing.
    press(&mut app, "Escape");
    pause(ctx, 300);
    let clean = rows_text(&mut app, &tab);
    key(&mut app, "AltRight", true);
    pause(ctx, 300);
    ctx.poll(8_000, &mut || active(&mut app));
    pause(ctx, 2_500);
    press(&mut app, "Escape");
    key(&mut app, "AltRight", false);
    let cancelled = ctx.until("the pill to go after Escape", 4_000, &mut || {
        !active(&mut app)
    });
    ctx.ok(cancelled, "Escape cancels a dictation");
    pause(ctx, 1_500);
    ctx.ok(rows_text(&mut app, &tab) == clean, "and nothing is pasted");

    // Off again: the server goes with the switch.
    let _ = set_switch(ctx, &mut app, "dictation-enabled", false);
    let stopped = ctx.until("the speech server to stop", 8_000, &mut || {
        ours(dictproc::speech_servers(&engine)) == 0
    });
    ctx.ok(stopped, "switching dictation off kills the speech server");
    let _ = app.close();
    let none = ctx.until("no speech server after the app", 8_000, &mut || {
        ours(dictproc::speech_servers(&engine)) == 0
    });
    ctx.ok(none, "and none outlives the app");
    Ok(())
}

/// PT `dictationPage` (36 assertions): Task 30.
pub fn dictation_page(ctx: &mut Ctx) -> Result<(), String> {
    ctx.not_ported(30)
}

/// PT `dictationParakeet` (11 assertions): Task 29. Parakeet is asked for auto, so the English sentence
/// comes back in English though the stored language is Norwegian; its language picker is disabled; no
/// Parakeet pass outlives its pass and no Whisper server ever starts.
pub fn dictation_parakeet(ctx: &mut Ctx) -> Result<(), String> {
    let engine = engine_dir(ctx.packaged);
    let model = fixtures::require(PARAKEET_FIXTURE)?;
    let clip = fixtures::require(CLIP_FIXTURE)?;
    let (profile, alpha) = world_paths(ctx);
    let dict_root = profile.join("dictation");
    stage_model(&dict_root, &model, "parakeet-v3-q4.bin")?;

    ctx.ok(
        engine.join("parakeet-cli.exe").is_file() && engine.join("parakeet.dll").is_file(),
        "Parakeet's runner is bundled beside the speech engine (vendor/whisper)",
    );
    seed(
        &profile,
        &[
            ("flare.dictation.hotkey", RIGHT_ALT),
            ("flare.dictation.model", "parakeet-v3-q4"),
            ("flare.dictation.language", "no"),
            ("flare.dictation.sounds", "0"),
            ("flare.dictation.pauseMedia", "0"),
        ],
    )?;
    let mut app = start(ctx, &dict_root, &clip, &engine, &alpha)?;
    let tab = shell_tab(&mut app);
    ctx.until(
        "the PowerShell prompt",
        prompt_timeout(Shell::PowerShell).as_millis() as u64,
        &mut || prompt_showing(&rows_text(&mut app, &tab), Shell::PowerShell),
    );

    let switched = set_switch(ctx, &mut app, "dictation-enabled", true);
    let lang = probe(&mut app, "dict.lang", Value::Null);
    ctx.ok(
        switched && lang["disabled"] == json!(true) && lang["limited"] == json!(true),
        "its language picker is disabled with the icon beside it",
    );
    // The warm-up runs a silent pass a moment after switching on, then ends. That the pass RUNS is the
    // engine tests' to prove; here only that nothing is left behind once it could have.
    pause(ctx, 6_000);
    let idle = ours(dictproc::parakeet_passes(&engine)) == 0
        && ours(dictproc::speech_servers(&engine)) == 0;
    ctx.ok(idle, "switched on, nothing stays running");
    back_to_shell(ctx, &mut app);
    click_grid(&mut app);

    key(&mut app, "AltRight", true);
    pause(ctx, 300);
    let listening = ctx.until("the pill to show Listening", 8_000, &mut || {
        pill_listening(&mut app)
    });
    ctx.ok(listening, "holding Right Alt opens the pill: Listening");
    let mut live = String::new();
    let got_live = ctx.until("live text to appear", 15_000, &mut || {
        live = live_text(&mut app);
        !live.trim().is_empty()
    });
    ctx.ok(
        got_live,
        format!(
            "live text appears while still listening (\"{}\")",
            live.trim()
        ),
    );
    pause(ctx, 9_000); // let the whole sentence play
    let before = rows_text(&mut app, &tab);
    let released = Instant::now();
    key(&mut app, "AltRight", false);
    let heard = ctx.until(
        "the spoken sentence on the prompt line",
        30_000,
        &mut || sentence_heard(&rows_text(&mut app, &tab)),
    );
    let ms = released.elapsed().as_millis();
    ctx.ok(
        heard,
        format!("the spoken sentence arrives on the prompt line ({ms} ms after release)"),
    );
    let gone = ctx.until("the pill to go", 5_000, &mut || !active(&mut app));
    ctx.ok(gone, "and the pill goes away");
    let after = rows_text(&mut app, &tab);
    ctx.ok(
        count_prompts(&after) == count_prompts(&before),
        "NO ENTER was sent: there is no new prompt, the text is still being edited",
    );
    let passes_done = ctx.until("the Parakeet pass to end", 4_000, &mut || {
        ours(dictproc::parakeet_passes(&engine)) == 0
    });
    ctx.ok(passes_done, "no Parakeet process outlives its pass");
    ctx.ok(
        ours(dictproc::speech_servers(&engine)) == 0,
        "and no Whisper server was ever started",
    );

    // Off again, and nothing at all.
    let _ = set_switch(ctx, &mut app, "dictation-enabled", false);
    let none = ctx.until("no speech process after switching off", 8_000, &mut || {
        ours(dictproc::parakeet_passes(&engine)) == 0
            && ours(dictproc::speech_servers(&engine)) == 0
    });
    ctx.ok(none, "switched off, no speech process exists");
    let _ = app.close();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_is_counted_once_per_prompt_text() {
        assert_eq!(count_prompts("PS C:\\a> ls\nPS C:\\a> "), 2);
        assert_eq!(count_prompts("no prompt here"), 0);
        assert_eq!(count_prompts("PS C:\\a"), 0);
        assert_eq!(count_prompts("PS >"), 1);
    }

    #[test]
    fn the_sentence_is_heard_across_line_breaks_and_any_case() {
        assert!(sentence_heard("Ask not\nwhat your   COUNTRY"));
        assert!(sentence_heard(
            "...ASK NOT WHAT YOUR COUNTRY can do for you"
        ));
        assert!(!sentence_heard("ask what your country"));
        assert!(!sentence_heard(""));
    }

    #[test]
    fn the_engine_folder_is_this_checkouts_vendor_whisper_with_no_dot_dot() {
        let dir = engine_dir(false);
        assert!(
            dir.ends_with(Path::new("vendor").join("whisper")),
            "{}",
            dir.display()
        );
        assert!(!dir.to_string_lossy().contains(".."));
        assert!(!dir.to_string_lossy().contains("tools"));
    }

    #[test]
    fn a_failed_process_list_counts_as_many_so_no_zero_check_passes() {
        assert_eq!(ours(Ok(0)), 0);
        assert_eq!(ours(Ok(1)), 1);
        assert_ne!(ours(Err("the list failed".into())), 0);
        assert_ne!(ours(Err("the list failed".into())), 1);
    }
}
