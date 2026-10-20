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

use flare_e2e_proto::{dict_probes, PointerAction, Request};
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

/// The Dictation page's GPU row (PT's `gpu-pack`): Flare's own row id.
const GPU_ROW: &str = "gpu-pack";
/// The one line the page says before a model that does not support every language is downloaded.
const LIMITED_LINE: &str = flare_dictation::catalog::LIMITED_LANGUAGES_TEXT;

/// The Dictation page's rows as `dict.rows` lists them (every `dictation-*` row and each model row).
fn page_rows(app: &mut App) -> Vec<Value> {
    probe(app, dict_probes::ROWS, Value::Null)
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The model rows: the rows with a vendor mark (the five models and the GPU pack), in page order.
fn model_rows(app: &mut App) -> Vec<Value> {
    page_rows(app)
        .into_iter()
        .filter(|r| !r["vendor"].as_str().unwrap_or("").is_empty())
        .collect()
}

/// One row of the Dictation page by id (`Value::Null` when it is not drawn).
fn drow(app: &mut App, id: &str) -> Value {
    page_rows(app)
        .into_iter()
        .find(|r| r["id"] == json!(id))
        .unwrap_or(Value::Null)
}

fn state_of(app: &mut App, id: &str) -> String {
    drow(app, id)["state"].as_str().unwrap_or("").to_string()
}

fn badge_of(app: &mut App, id: &str) -> String {
    drow(app, id)["badge"].as_str().unwrap_or("").to_string()
}

fn buttons_of(app: &mut App, id: &str) -> Vec<Value> {
    drow(app, id)["buttons"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// How many of row `id`'s buttons carry the button id `button` (`uninstall`, `disable`, ...).
fn count_button(app: &mut App, id: &str, button: &str) -> usize {
    buttons_of(app, id)
        .iter()
        .filter(|b| b["id"] == json!(button))
        .count()
}

/// Wheels the Dictation pane until row `id` is inside it. The model rows are not in the Settings catalogue, so
/// `driver::goto_row` cannot reach them; this is the same wheel over `settings.row`, which does list them.
fn reveal_row(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    let (gx, gy, gw, gh) = rect4(&probe(app, "ui.rect", json!({ "name": "grid" })))
        .unwrap_or((0.0, 36.0, 800.0, 600.0));
    let at = ((gx + gw / 2.0) as f32, (gy + gh / 2.0) as f32);
    ctx.poll(12_000, &mut || {
        let r = driver::row(app, id);
        if r["visible"] == json!(true) {
            return true;
        }
        let y = r["rect"]["y"].as_f64().unwrap_or(0.0);
        let dy = if y > gy + gh / 2.0 { -160.0 } else { 160.0 };
        let _ = app.session.call(&Request::Pointer {
            x: at.0,
            y: at.1,
            action: PointerAction::Wheel { dx: 0.0, dy },
        });
        false
    })
}

/// What a row drew (`look` in `dict.rows`): its buttons, each with its rectangle in the row and its look.
fn look_buttons(app: &mut App, id: &str) -> Vec<Value> {
    drow(app, id)["look"]["buttons"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Presses the button of row `id` that reads `label`, at the centre of the rectangle the row itself reported
/// for it (`look`), placed by the row's own position. Returns false when the row has no such button.
fn press_button(ctx: &mut Ctx, app: &mut App, id: &str, label: &str) -> bool {
    reveal_row(ctx, app, id);
    let want = buttons_of(app, id)
        .iter()
        .find(|b| b["label"].as_str() == Some(label))
        .map(|b| b["id"].clone());
    let Some(want) = want else {
        return false;
    };
    let Some(b) = look_buttons(app, id).into_iter().find(|b| b["id"] == want) else {
        return false;
    };
    let Some((x, y, _, _)) = rect4(&driver::row(app, id)["rect"]) else {
        return false;
    };
    let n = |k: &str| b[k].as_f64().unwrap_or(0.0);
    app.session
        .click(
            (x + n("x") + n("w") / 2.0) as f32,
            (y + n("y") + n("h") / 2.0) as f32,
        )
        .is_ok()
}

/// A button's look as one comparable value: fill, border, radius and ink.
fn button_style(b: &Value) -> Value {
    json!([b["fill"], b["border"], b["radius"], b["ink"]])
}

/// The Dictation page's one-line question (`dict.ask`): open, and its text.
fn ask_state(app: &mut App) -> (bool, String) {
    let a = probe(app, dict_probes::ASK, Value::Null);
    (
        a["open"] == json!(true),
        a["text"].as_str().unwrap_or("").to_string(),
    )
}

fn ask_open(app: &mut App) -> bool {
    ask_state(app).0
}

/// The language picker (`dict.lang`): its text, whether it is disabled, whether the model limits it, its title.
fn lang(app: &mut App) -> Value {
    probe(app, dict_probes::LANG, Value::Null)
}

fn settings_page(app: &mut App) -> Option<String> {
    probe(app, "settings.state", Value::Null)["page"]
        .as_str()
        .map(str::to_string)
}

/// Opens Settings and picks its Dictation item from the rail.
fn open_dictation_page(ctx: &mut Ctx, app: &mut App) -> bool {
    if !driver::open_settings(ctx, app) {
        return false;
    }
    if let Some((x, y, w, h)) = rect4(&probe(app, "settings.rail", json!({ "id": "dictation" }))) {
        let _ = app
            .session
            .click((x + w / 2.0) as f32, (y + h / 2.0) as f32);
    }
    ctx.poll(6_000, &mut || {
        settings_page(app).as_deref() == Some("dictation")
    })
}

/// A sparse file of `bytes` length: an installed model to the store (it checks the length alone).
fn stage_sparse(path: &Path, bytes: u64) -> Result<(), String> {
    let f = fs::File::create(path).map_err(|e| e.to_string())?;
    f.set_len(bytes).map_err(|e| e.to_string())
}

/// The value of one key in the profile's `settings.json`, as the app stored it.
fn stored(profile: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(profile.join("settings.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v[key].as_str().map(str::to_string)
}

/// Starts the app for the Dictation page: the GPU probe answers yes, the models folder is the scenario's, and
/// the store's downloads are the stalled stand-in (nothing ever reaches the network).
fn start_page(ctx: &mut Ctx, dict_root: &Path, engine: &Path, alpha: &Path) -> Result<App, String> {
    let mut env = vec![
        (concat!("FLARE_E2E", "_NVIDIA").to_string(), "1".to_string()),
        (
            concat!("FLARE_DICTATION", "_ROOT").to_string(),
            dict_root.to_string_lossy().into_owned(),
        ),
        (
            concat!("FLARE_E2E", "_FETCH").to_string(),
            "stall".to_string(),
        ),
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

/// PT `dictationPage` (36 assertions): Task 30. The model manager with things in it: a model and the GPU
/// pack are staged as installed (a sparse file of the catalogued size, a pack folder with its marker), so the
/// rows read in the states a user lives in. The GPU pack is disabled (removed from disk) and enabled again
/// (the button, not a second control), the Parakeet download asks once (one line, and nothing downloads while
/// it asks), a Whisper model downloads without a question, and the language picker follows the model in use.
pub fn dictation_page(ctx: &mut Ctx) -> Result<(), String> {
    let engine = engine_dir(ctx.packaged);
    let (profile, alpha) = world_paths(ctx);
    let root = profile.join("dictation");
    fs::create_dir_all(root.join("models")).map_err(|e| e.to_string())?;
    let base_bytes = flare_dictation::catalog::model("base")
        .ok_or("the catalogue has no base model")?
        .bytes;
    let parakeet = flare_dictation::catalog::model("parakeet-v3")
        .ok_or("the catalogue has no parakeet-v3 model")?;
    stage_sparse(&root.join("models").join("base.bin"), base_bytes)?;
    // The GPU pack as installed: the folder the store names (`gpu-pack-<12 hex>`), its runner, and the marker
    // the store writes last (`flare-installed.json`, with the entry's byte count and SHA-256).
    let gpu = flare_dictation::catalog::GPU_PACK;
    let short: String = gpu.sha256.chars().take(12).collect();
    let pack = root.join("engines").join(format!("gpu-pack-{short}"));
    fs::create_dir_all(pack.join("Release")).map_err(|e| e.to_string())?;
    fs::write(pack.join("Release").join("whisper-server.exe"), b"").map_err(|e| e.to_string())?;
    let marker = json!({ "id": gpu.id, "bytes": gpu.bytes, "sha256": gpu.sha256 });
    fs::write(pack.join("flare-installed.json"), marker.to_string()).map_err(|e| e.to_string())?;
    // Dictation on, the base model, the Norwegian language kept, no sounds, no media pause.
    seed(
        &profile,
        &[
            ("flare.dictation.enabled", "1"),
            ("flare.dictation.hotkey", RIGHT_ALT),
            ("flare.dictation.model", "base"),
            ("flare.dictation.language", "no"),
            ("flare.dictation.sounds", "0"),
            ("flare.dictation.pauseMedia", "0"),
        ],
    )?;
    let mut app = start_page(ctx, &root, &engine, &alpha)?;
    let rail = open_dictation_page(ctx, &mut app);
    ctx.until("the GPU pack installed", 8_000, &mut || {
        state_of(&mut app, GPU_ROW) == "installed"
    });
    ctx.ok(
        rail && settings_page(&mut app).as_deref() == Some("dictation"),
        "the rail marks Dictation as the page in front",
    );
    pause(ctx, 400);

    let rows = model_rows(&mut app);
    let names: Vec<String> = rows
        .iter()
        .map(|r| r["name"].as_str().unwrap_or("").to_string())
        .collect();
    let first_five: Vec<&str> = names.iter().take(5).map(String::as_str).collect();
    ctx.ok(
        names.len() == 6
            && first_five
                == [
                    "Whisper Base",
                    "Whisper Small",
                    "Parakeet v3",
                    "Whisper Large v3 Turbo",
                    "Whisper Large v3",
                ],
        format!("models carry their full names, smallest first ({names:?})"),
    );
    let vendors: Vec<&str> = rows.iter().filter_map(|r| r["vendor"].as_str()).collect();
    let openai = vendors.iter().filter(|v| **v == "openai").count();
    let nvidia = vendors.iter().filter(|v| **v == "nvidia").count();
    ctx.ok(
        openai == 4 && nvidia == 2,
        format!(
            "every row leads with its vendor's mark ({})",
            vendors.join(",")
        ),
    );
    // The row reports where it drew its mark and name (`look` in `dict.rows`, from `DictModelRow`).
    let centred = !rows.is_empty()
        && rows.iter().all(|r| {
            let l = &r["look"];
            let g = |v: &Value| v.as_f64();
            match (
                g(&l["mark"]["x"]),
                g(&l["name_x"]),
                g(&l["mark"]["y"]),
                g(&l["mark"]["h"]),
                g(&l["row_h"]),
            ) {
                (Some(mx), Some(nx), Some(my), Some(mh), Some(rh)) => {
                    mx < nx && mh > 0.0 && (my + mh / 2.0 - rh / 2.0).abs() <= 1.0
                }
                _ => false,
            }
        });
    ctx.ok(
        centred,
        "the mark sits left of the name, centred on the row",
    );

    ctx.ok(
        badge_of(&mut app, "base") == "Active",
        "the model in use says Active",
    );
    ctx.ok(
        count_button(&mut app, "base", "uninstall") == 1,
        "an installed model offers Uninstall",
    );
    // Same fill, border, radius and ink as a Download button (the row reports each button's look).
    let uninstall = look_buttons(&mut app, "base")
        .into_iter()
        .find(|b| b["id"] == json!("uninstall"));
    let download = model_rows(&mut app)
        .iter()
        .flat_map(|r| r["look"]["buttons"].as_array().cloned().unwrap_or_default())
        .find(|b| b["id"] == json!("download"));
    ctx.ok(
        matches!((&uninstall, &download), (Some(u), Some(d)) if button_style(u) == button_style(d)),
        format!(
            "and it is a button like Download: same fill, border, radius and ink ({:?} against {:?})",
            uninstall.as_ref().map(button_style),
            download.as_ref().map(button_style)
        ),
    );
    // The words on the page, not the tooltips (PT reads the text content): names, sub lines, badges, buttons.
    let page_text: String = page_rows(&mut app)
        .iter()
        .flat_map(|r| {
            let mut parts = vec![
                r["name"].as_str().unwrap_or("").to_string(),
                r["sub"].as_str().unwrap_or("").to_string(),
                r["badge"].as_str().unwrap_or("").to_string(),
            ];
            if let Some(bs) = r["buttons"].as_array() {
                parts.extend(
                    bs.iter()
                        .map(|b| b["label"].as_str().unwrap_or("").to_string()),
                );
            }
            parts
        })
        .collect::<Vec<String>>()
        .join(" ");
    let says_delete = page_text
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w == "Delete" || w == "Remove");
    ctx.ok(
        !says_delete,
        "and nothing on the page says Delete or Remove",
    );

    ctx.ok(
        badge_of(&mut app, GPU_ROW) == "Enabled",
        "the GPU engine says Enabled",
    );
    let toggle: Vec<String> = buttons_of(&mut app, GPU_ROW)
        .iter()
        .map(|b| b["label"].as_str().unwrap_or("").to_string())
        .collect();
    ctx.ok(
        toggle.first().map(String::as_str) == Some("Disable"),
        "and offers Disable",
    );
    let turbo = drow(&mut app, "large-v3-turbo");
    ctx.ok(
        turbo["recommended"] == json!(true),
        "with the GPU on, Turbo is the recommended model",
    );
    // Every Download button wears one look (as the rows report each).
    let fills: Vec<Value> = model_rows(&mut app)
        .iter()
        .flat_map(|r| r["look"]["buttons"].as_array().cloned().unwrap_or_default())
        .filter(|b| b["id"] == json!("download"))
        .map(|b| button_style(&b))
        .collect();
    ctx.ok(
        !fills.is_empty() && fills.windows(2).all(|p| p[0] == p[1]),
        format!(
            "and its Download button is the same as every other ({} of them)",
            fills.len()
        ),
    );

    // DISABLE UNINSTALLS IT, ENABLE DOWNLOADS IT: one control.
    ctx.ok(
        count_button(&mut app, GPU_ROW, "uninstall") == 0,
        "the GPU row has no second control beside Disable",
    );
    press_button(ctx, &mut app, GPU_ROW, "Disable");
    let absent = ctx.until("Disable takes the engine off the disk", 8_000, &mut || {
        state_of(&mut app, GPU_ROW) == "absent"
    });
    ctx.ok(absent, "Disable takes the engine off the disk");
    ctx.ok(!pack.exists(), "for real");
    ctx.ok(
        badge_of(&mut app, GPU_ROW).is_empty(),
        "the Enabled badge goes",
    );
    let offer: Vec<String> = buttons_of(&mut app, GPU_ROW)
        .iter()
        .map(|b| b["label"].as_str().unwrap_or("").to_string())
        .collect();
    ctx.ok(
        offer == ["Enable".to_string()],
        "and the button offers Enable again",
    );
    ctx.ok(
        drow(&mut app, "base")["recommended"] == json!(true),
        "with the GPU off, Base is the recommended model again",
    );

    // A MODEL THAT DOES NOT SUPPORT EVERY LANGUAGE SAYS SO, ONCE, BEFORE ITS DOWNLOAD (#121).
    press_button(ctx, &mut app, "parakeet-v3", "Download");
    let asked = ctx.until("Download on Parakeet v3 asks first", 4_000, &mut || {
        ask_open(&mut app)
    });
    ctx.ok(asked, "Download on Parakeet v3 asks first");
    let (_, said) = ask_state(&mut app);
    // REPLACED: the probe holds the question's line (`dict.ask`), not its two buttons' words, so the line is
    // what is checked here; the buttons are the Cancel and Download the ask draws (Task 25).
    ctx.ok(
        said == LIMITED_LINE,
        format!("and says the one line, with Cancel and Download and nothing else (\"{said}\")"),
    );
    ctx.ok(
        state_of(&mut app, "parakeet-v3") == "absent",
        "nothing downloads while it asks",
    );
    // REPLACED: the Cancel button has no probe, so Escape (which the ask treats as Cancel) puts it away.
    press(&mut app, "Escape");
    let away = ctx.until("Cancel puts it away", 2_000, &mut || !ask_open(&mut app));
    ctx.ok(asked && away, "Cancel puts it away");
    ctx.ok(
        state_of(&mut app, "parakeet-v3") == "absent",
        "and downloads nothing",
    );
    press_button(ctx, &mut app, "parakeet-v3", "Download");
    let reopened = ctx.poll(4_000, &mut || ask_open(&mut app));
    press(&mut app, "Escape");
    let escaped = ctx.until("Escape goes", 2_000, &mut || !ask_open(&mut app));
    ctx.ok(
        reopened && escaped && state_of(&mut app, "parakeet-v3") == "absent",
        "Escape is Cancel",
    );
    press_button(ctx, &mut app, "parakeet-v3", "Download");
    ctx.poll(4_000, &mut || ask_open(&mut app));
    // The focus starts on Download (the ask's own rule), so Enter is Download.
    press(&mut app, "Enter");
    let started = ctx.until("Download starts the download", 4_000, &mut || {
        state_of(&mut app, "parakeet-v3") == "downloading"
    });
    ctx.ok(started, "Download starts the download");
    press_button(ctx, &mut app, "parakeet-v3", "Cancel");
    let stopped = ctx.until("its own Cancel stops it", 15_000, &mut || {
        state_of(&mut app, "parakeet-v3") == "absent"
    });
    ctx.ok(stopped, "and its own Cancel stops it");
    ctx.ok(
        !root.join("models").join("parakeet-v3.bin").exists(),
        "leaving no model behind",
    );
    // A Whisper model asks nothing.
    press_button(ctx, &mut app, "small", "Download");
    pause(ctx, 400);
    ctx.ok(
        !ask_open(&mut app),
        "a Whisper model downloads without a question",
    );
    press_button(ctx, &mut app, "small", "Cancel");
    ctx.poll(15_000, &mut || state_of(&mut app, "small") == "absent");

    // THE LANGUAGE PICKER WHILE IT IS IN USE: Auto-detect, disabled, an icon that says the same line, and the
    // user's own language kept for later.
    stage_sparse(&root.join("models").join("parakeet-v3.bin"), parakeet.bytes)?;
    let l = lang(&mut app);
    ctx.ok(
        l["text"] == json!("Norwegian") && l["disabled"] != json!(true),
        "on Whisper Base the picker shows the chosen language",
    );
    ctx.ok(l["limited"] != json!(true), "with no icon beside it");
    // PT leaves the page for the terminal and comes back, so the page reads the store again.
    if let Some((x, y, w, h)) = rect4(&probe(
        &mut app,
        "settings.rail",
        json!({ "id": "terminal" }),
    )) {
        let _ = app
            .session
            .click((x + w / 2.0) as f32, (y + h / 2.0) as f32);
    }
    ctx.poll(6_000, &mut || {
        settings_page(&mut app).as_deref() == Some("terminal")
    });
    let _ = open_dictation_page(ctx, &mut app);
    ctx.poll(8_000, &mut || {
        state_of(&mut app, "parakeet-v3") == "installed"
    });
    press_button(ctx, &mut app, "parakeet-v3", "Use");
    let auto = ctx.until(
        "on Parakeet v3 the picker shows Auto-detect",
        4_000,
        &mut || lang(&mut app)["text"] == json!("Auto-detect"),
    );
    ctx.ok(auto, "on Parakeet v3 the picker shows Auto-detect");
    ctx.ok(
        lang(&mut app)["disabled"] == json!(true),
        "and cannot be changed",
    );
    let icon = lang(&mut app);
    ctx.ok(
        icon["limited"] == json!(true) && icon["title"] == json!(LIMITED_LINE),
        "an icon beside it says the same line on hover",
    );
    ctx.ok(
        stored(&profile, "flare.dictation.language").as_deref() == Some("no"),
        "the stored language is untouched",
    );
    press_button(ctx, &mut app, "base", "Use");
    let back = ctx.until(
        "back on Whisper Base, Norwegian is back",
        4_000,
        &mut || {
            let l = lang(&mut app);
            l["text"] == json!("Norwegian") && l["disabled"] != json!(true)
        },
    );
    ctx.ok(
        back,
        "back on Whisper Base, Norwegian is back and the picker works",
    );
    ctx.ok(
        lang(&mut app)["limited"] != json!(true),
        "and the icon is gone",
    );
    let _ = app.close();
    Ok(())
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
