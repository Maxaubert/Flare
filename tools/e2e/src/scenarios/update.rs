//! The update chip and window scenarios (Task 39): updateWindow, updateGuard, updateNotes.
//!
//! PT: run.mjs:7222-7555 (updateWindow, 78 checks), 7556-7653 (updateGuard, 26) and 7654-7721
//! (updateNotes, 9). Prism Terminal reads the page's DOM; Flare answers through the `update.window`
//! probe (the window's own properties and boxes), `app.state` (the tabs and the close question),
//! `theme.tokens` and snapshots read off the disk for what only pixels tell.
//!
//! The seams are M9's: `--preview-update`, `FLARE_E2E_UPDATE_OFFER` and `_NOTES`, and the files the
//! app writes into its profile folder (`update-state.json`, `update-calls.json`) and reads there
//! (`update-command`). The plan's text puts them under `<profile>\e2e`; the app writes them in the
//! profile folder itself (`inst.profile.dir`), so the harness reads both places.
//!
//! Where Flare differs from the page, a body says so:
//! - There is no bridge a page could call, so "an install for any url but the offered one is
//!   refused" is driven through the only outside input there is, the command file. A command that
//!   names a url is not one of its three words, so it must do nothing.
//! - There is no element tree: "plain elements only" and "no handler ran" become "the strings hold
//!   no markup" and "nothing was opened".

use std::path::{Path, PathBuf};
use std::process::Command;

use flare_e2e_proto::{PointerAction, Request};
use serde_json::{json, Value};

use super::help::Img;
use crate::ctx::{App, Ctx, Launch};
use crate::pixels::{contrast, lum, Rgb};
use crate::standins::agent_cmd::{FakeClaude, Step};
use crate::standins::hooklines::hook_line;
use crate::world::{prompt_timeout, rows_text, wait_for_prompt, Shell};

/// `FLARE_E2E_UPDATE_OFFER`: a real-shaped offer whose installer the app refuses before a byte is sent.
const OFFER_VAR: &str = concat!("FLARE_E2E", "_UPDATE_OFFER");
/// `FLARE_E2E_UPDATE_NOTES`: the release body that offer carries.
const NOTES_VAR: &str = concat!("FLARE_E2E", "_UPDATE_NOTES");
/// The offer the guard scenarios hand the app.
const OFFER_URL: &str = "https://example.invalid/Flare-Setup-x64-99.0.0.exe";
/// What the preview's install ends on, in the window.
const PREVIEW_DONE: &str = "Preview only: nothing was installed";
/// The idle title Claude sets (U+2733, the sparkle) and the working one (U+25D0, the half circle).
const IDLE_TITLE: &str = "\u{2733} Claude Code";
const WORKING_TITLE: &str = "\u{25D0} Claude Code";

// ---- small readers --------------------------------------------------------------------------------------

/// A probe's answer, `Null` when it fails (a check on it then reads false).
fn probe(app: &mut App, name: &str, arg: Value) -> Value {
    app.session.probe(name, arg).unwrap_or(Value::Null)
}

/// The `update.window` answer.
fn uw(app: &mut App) -> Value {
    probe(app, "update.window", Value::Null)
}

/// A rectangle value (`x`, `y`, `w`, `h`) as a tuple, in css pixels.
fn rc(v: &Value) -> Option<(f64, f64, f64, f64)> {
    Some((
        v["x"].as_f64()?,
        v["y"].as_f64()?,
        v["w"].as_f64()?,
        v["h"].as_f64()?,
    ))
}

/// Clicks the centre of a rectangle value; false when there is none.
fn click_v(app: &mut App, v: &Value) -> bool {
    match rc(v) {
        Some((x, y, w, h)) => app
            .session
            .click((x + w / 2.0) as f32, (y + h / 2.0) as f32)
            .is_ok(),
        None => false,
    }
}

/// The scale of the app (device pixels per css pixel).
fn scale_of(app: &mut App) -> f64 {
    app.session
        .state()
        .map(|s| f64::from(s.scale))
        .unwrap_or(1.0)
}

/// True while the update window is up.
fn is_up(app: &mut App) -> bool {
    uw(app)["open"] == json!(true)
}

/// The chip part of the answer.
fn chip_of(app: &mut App) -> Value {
    uw(app)["chip"].clone()
}

/// The chip is showing.
fn chip_shown(app: &mut App) -> bool {
    chip_of(app)["shown"] == json!(true)
}

/// Clicks the chip, as a person does.
fn click_chip(app: &mut App) -> bool {
    let c = chip_of(app);
    click_v(app, &c["rect"])
}

/// Clicks the chip and waits until the window is up.
fn open_window(ctx: &mut Ctx, app: &mut App) -> bool {
    click_chip(app);
    ctx.until("the update window to open", 4_000, &mut || is_up(app))
}

/// Waits until the window has gone.
fn closed(ctx: &mut Ctx, app: &mut App) -> bool {
    ctx.until("the update window to close", 4_000, &mut || !is_up(app))
}

/// Presses Install in the window.
fn click_install(app: &mut App) -> bool {
    let w = uw(app);
    click_v(app, &w["install"])
}

/// Presses the left button (Cancel, or Close once an ending is said).
fn click_cancel(app: &mut App) -> bool {
    let w = uw(app);
    click_v(app, &w["cancel"])
}

/// The progress, 0 to 100.
fn pct_of(app: &mut App) -> i64 {
    uw(app)["pct"].as_i64().unwrap_or(-1)
}

/// The phase word the window shows: `idle`, `downloading` or `installing`.
fn phase_of(app: &mut App) -> String {
    uw(app)["phase"].as_str().unwrap_or("").to_string()
}

/// The status line.
fn status_of(app: &mut App) -> String {
    uw(app)["status"].as_str().unwrap_or("").to_string()
}

/// The labels of the two buttons, with `(off)` after one that cannot be pressed.
fn buttons_of(v: &Value) -> String {
    v["buttons"]
        .as_array()
        .map(|b| {
            b.iter()
                .map(|x| {
                    let off = if x["enabled"] == json!(false) {
                        " (off)"
                    } else {
                        ""
                    };
                    format!("{}{off}", x["label"].as_str().unwrap_or(""))
                })
                .collect::<Vec<_>>()
                .join("|")
        })
        .unwrap_or_default()
}

/// The sections as `(heading, items)`.
fn sections_of(v: &Value) -> Vec<(String, Vec<String>)> {
    v["sections"]
        .as_array()
        .map(|s| {
            s.iter()
                .map(|x| {
                    (
                        x["name"].as_str().unwrap_or("").to_string(),
                        x["items"]
                            .as_array()
                            .map(|i| {
                                i.iter()
                                    .map(|e| e.as_str().unwrap_or("").to_string())
                                    .collect()
                            })
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every entry of every section, in order.
fn entries_of(v: &Value) -> Vec<String> {
    sections_of(v)
        .into_iter()
        .flat_map(|(_, items)| items)
        .collect()
}

/// All the words the window shows, as one string.
fn all_text(v: &Value) -> String {
    let mut out = vec![
        v["title"].as_str().unwrap_or("").to_string(),
        v["subtitle"].as_str().unwrap_or("").to_string(),
        v["more"].as_str().unwrap_or("").to_string(),
        v["empty_note"].as_str().unwrap_or("").to_string(),
        v["status"].as_str().unwrap_or("").to_string(),
    ];
    for (name, items) in sections_of(v) {
        out.push(name);
        out.extend(items);
    }
    out.join("\n")
}

/// The next minor after `current`, as the preview offers it: `{major}.{minor+1}.0`.
pub fn next_minor(current: &str) -> String {
    let mut parts = current
        .trim_start_matches(['v', 'V'])
        .split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    format!("{major}.{}.0", minor + 1)
}

/// True when a line ends in a pull request number, `(#123)`.
pub fn ends_with_pr_number(s: &str) -> bool {
    let t = s.trim_end();
    let Some(rest) = t.strip_suffix(')') else {
        return false;
    };
    match rest.rfind("(#") {
        Some(at) => {
            let digits = &rest[at + 2..];
            !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

/// True when a line starts like a commit subject: lower-case letters, an optional `(scope)`, an
/// optional `!`, then a colon.
pub fn starts_with_commit_type(s: &str) -> bool {
    let word = s.chars().take_while(char::is_ascii_lowercase).count();
    if word == 0 {
        return false;
    }
    let mut rest = &s[word..];
    if let Some(r) = rest.strip_prefix('(') {
        match r.find(')') {
            Some(close) => rest = &r[close + 1..],
            None => return false,
        }
    }
    let rest = rest.strip_prefix('!').unwrap_or(rest);
    rest.starts_with(':')
}

/// The two numbers of `update-calls.json`: `(checks, installs)`; `None` when the file is not there yet.
fn calls(app: &App) -> Option<(u64, u64)> {
    let v = state_file(&app.profile, "update-calls.json")?;
    Some((v["checks"].as_u64()?, v["installs"].as_u64()?))
}

/// One of the app's e2e files, from the profile folder or its `e2e` subfolder.
fn state_file(profile: &Path, name: &str) -> Option<Value> {
    [profile.join(name), profile.join("e2e").join(name)]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
}

/// Hands the app a one-word command through its command file.
fn command(app: &App, text: &str) -> bool {
    // Temp file then rename, so the app never reads half a word.
    let tmp = app.profile.join("update-command.tmp");
    std::fs::write(&tmp, text).is_ok()
        && std::fs::rename(&tmp, app.profile.join("update-command")).is_ok()
}

/// True while the command file is still there (the app deletes it when it has read it).
fn command_pending(app: &App) -> bool {
    app.profile.join("update-command").exists()
}

// ---- pixels -----------------------------------------------------------------------------------------------

/// The rgb of a `#rrggbb` or `#rrggbbaa` string (black when it is not one).
fn rgb_of_hex(hex: &str) -> Rgb {
    let h = hex.trim_start_matches('#');
    let ch = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    [ch(0), ch(2), ch(4)]
}

/// The theme's tokens (`theme.tokens`).
fn tokens(app: &mut App) -> Value {
    probe(app, "theme.tokens", Value::Null)
}

/// The colour of the css point (x, y) in a snapshot taken at `k` device pixels per css pixel.
fn px(img: &Img, k: f64, x: f64, y: f64) -> Rgb {
    img.at(
        (x * k).round().max(0.0) as usize,
        (y * k).round().max(0.0) as usize,
    )
}

/// True when two colours differ by at most `tol` in every channel.
fn near(a: Rgb, b: Rgb, tol: u8) -> bool {
    (0..3).all(|i| a[i].abs_diff(b[i]) <= tol)
}

/// The best contrast any pixel of the css region (x0, y0, x1, y1) has against `ground`.
fn best_contrast(img: &Img, k: f64, region: (f64, f64, f64, f64), ground: Rgb) -> f64 {
    let (x0, y0) = ((region.0 * k) as usize, (region.1 * k) as usize);
    let (x1, y1) = ((region.2 * k) as usize, (region.3 * k) as usize);
    let mut best = 1.0_f64;
    for y in y0..y1 {
        for x in x0..x1 {
            best = best.max(contrast(img.at(x, y), ground));
        }
    }
    best
}

/// Takes a snapshot and decodes it; the scale it was taken at comes with it.
fn look(ctx: &mut Ctx, app: &mut App, name: &str) -> Option<(Img, f64)> {
    let path = ctx.shot(app, name)?;
    let img = Img::load(&path)?;
    Some((img, scale_of(app)))
}

/// A point of the dialog's own surface, well away from any text: 8 css px inside its left edge, at
/// the given height of the box.
fn surface_at(w: &Value, frac: f64) -> Option<(f64, f64)> {
    let (x, y, _, h) = rc(&w["box"])?;
    Some((x + 8.0, y + h * frac))
}

// ---- what a window or a chip looked like at one moment -------------------------------------------------------

/// One sample of the window and the chip, taken every poll while an install runs.
#[derive(Debug, Clone)]
struct Sample {
    up: bool,
    /// The box as device pixels `w x h @ x,y`, so a one-pixel change shows.
    boxed: String,
    phase: String,
    title: String,
    pct: i64,
    pct_text: String,
    status: String,
    cancel_label: String,
    cancel_on: bool,
    install_on: bool,
    chip_w: f64,
    chip_x: f64,
    chip_label: String,
    notice: String,
}

/// Reads one sample.
fn sample(app: &mut App) -> Sample {
    let w = uw(app);
    let b = &w["box"];
    Sample {
        up: w["open"] == json!(true),
        boxed: format!("{}x{}@{},{}", b["dw"], b["dh"], b["dx"], b["dy"]),
        phase: w["phase"].as_str().unwrap_or("").to_string(),
        title: w["title"].as_str().unwrap_or("").to_string(),
        pct: w["pct"].as_i64().unwrap_or(-1),
        pct_text: w["pct_text"].as_str().unwrap_or("").to_string(),
        status: w["status"].as_str().unwrap_or("").to_string(),
        cancel_label: w["buttons"][0]["label"].as_str().unwrap_or("").to_string(),
        cancel_on: w["buttons"][0]["enabled"] == json!(true),
        install_on: w["buttons"][1]["enabled"] == json!(true),
        chip_w: w["chip"]["rect"]["w"].as_f64().unwrap_or(-1.0),
        chip_x: w["chip"]["rect"]["x"].as_f64().unwrap_or(-1.0),
        chip_label: w["chip"]["label"].as_str().unwrap_or("").to_string(),
        notice: w["notice"].as_str().unwrap_or("").to_string(),
    }
}

/// Polls until `done` holds, taking a sample on every poll. False (and a FAIL) on a timeout.
fn sampling(
    ctx: &mut Ctx,
    app: &mut App,
    out: &mut Vec<Sample>,
    what: &str,
    timeout_ms: u64,
    done: &dyn Fn(&Sample) -> bool,
) -> bool {
    ctx.until(what, timeout_ms, &mut || {
        let s = sample(app);
        let hit = done(&s);
        out.push(s);
        hit
    })
}

/// True when `cond` never holds for `ms` (a "nothing changed" proof, polled every 50 ms).
fn never(ctx: &mut Ctx, app: &mut App, ms: u64, cond: &mut dyn FnMut(&mut App) -> bool) -> bool {
    let mut left = ms;
    while left > 0 {
        if cond(app) {
            return false;
        }
        ctx.settle(50);
        left = left.saturating_sub(50);
    }
    !cond(app)
}

/// The names in `dir` that start with `prefix`, sorted and joined.
fn names_with(dir: &Path, prefix: &str) -> String {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.to_lowercase().starts_with(prefix))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names.join("|")
}

/// How many other processes have `needle` on their command line. The query's own PowerShell names it
/// too, so it leaves itself out (PT does the same). `None` when the query fails.
fn processes_naming(needle: &str) -> Option<u32> {
    #[cfg(target_os = "macos")]
    use crate::proc::CommandExt;
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    let script = format!(
        "@(Get-CimInstance Win32_Process | Where-Object {{ $_.ProcessId -ne $PID -and $_.CommandLine -like '*{needle}*' }}).Count"
    );
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &script])
        .creation_flags(0x0800_0000)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

// ---- updateWindow ---------------------------------------------------------------------------------------------

/// PT: run.mjs:7222-7555. The window opened from the chip of a preview: the chip, the notes, every
/// way out, and an install sampled the whole way through.
pub fn update_window(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let mut app = ctx.launch(Launch {
        args: vec!["--preview-update".into()],
        folders: vec![alpha],
        renderer: Some("software"),
        ..Launch::default()
    })?;
    window_checks(ctx, &mut app);
    app.close();
    Ok(())
}

fn window_checks(ctx: &mut Ctx, app: &mut App) {
    let version = app.hello().map(|h| h.version).unwrap_or_default();
    let next = next_minor(&version);
    ctx.until("the one tab", 15_000, &mut || {
        app.session
            .state()
            .map(|s| s.tabs.len() == 1)
            .unwrap_or(false)
    });

    // PT: run.mjs:7258. The chip is in the title bar under --e2e too.
    let shown = ctx.until("the chip", 8_000, &mut || chip_shown(app));
    ctx.ok(
        shown,
        "with --preview-update the chip is in the title bar, under --e2e too",
    );
    // PT: run.mjs:7259.
    let label = chip_of(app)["label"].as_str().unwrap_or("").to_string();
    ctx.ok(
        label == format!("Update {next}"),
        format!("it offers the next minor after {version} (\"{label}\")"),
    );
    // PT: run.mjs:7260.
    ctx.ok(!is_up(app), "and nothing opens by itself");
    let chip0 = chip_of(app);
    let width0 = chip0["rect"]["w"].as_f64().unwrap_or(-1.0);
    let left0 = chip0["rect"]["x"].as_f64().unwrap_or(-1.0);

    // PT: run.mjs:7298-7300. Filled in the accent, read off the pixels: the fill is the theme's accent
    // and not a grey, and the label's ink clears the floor for small text.
    let tk = tokens(app);
    let sel_bg = rgb_of_hex(tk["sel_bg"].as_str().unwrap_or("#000000"));
    let on_accent = rgb_of_hex(tk["on_accent"].as_str().unwrap_or("#ffffff"));
    let snap = look(ctx, app, "chip-dark");
    let fill = snap.as_ref().map(|(img, k)| {
        let r = rc(&chip0["rect"]).unwrap_or((0.0, 0.0, 0.0, 0.0));
        px(img, *k, r.0 + 3.0, r.1 + r.3 / 2.0)
    });
    ctx.ok(
        fill.is_some_and(|f| near(f, sel_bg, 2)),
        format!(
            "the chip is filled in the accent, its label in the accent's ink ({fill:?} against {sel_bg:?})"
        ),
    );
    let chroma = fill
        .map(|f| f.iter().max().copied().unwrap_or(0) - f.iter().min().copied().unwrap_or(0))
        .unwrap_or(0);
    ctx.ok(
        chroma >= 40,
        format!("which is a colour and not the grey pill it was (channel spread {chroma})"),
    );
    let ink = contrast(on_accent, sel_bg);
    ctx.ok(
        ink >= 4.5,
        format!("and the label reads on it ({ink:.1}:1)"),
    );
    // PT: run.mjs:7301-7307. The leftmost of the controls at the right of the title bar.
    let chip_right = rc(&chip0["rect"]).map(|r| r.0 + r.2).unwrap_or(f64::MAX);
    let lefts: Vec<f64> = ["close", "max", "min", "cog", "help"]
        .iter()
        .filter_map(|n| rc(&probe(app, "ui.rect", json!({ "name": n }))).map(|r| r.0))
        .collect();
    ctx.ok(
        !lefts.is_empty() && lefts.iter().all(|l| *l >= chip_right - 0.5),
        "it is the LEFTMOST of the controls at the right of the title bar",
    );

    // What an install would leave behind, read BEFORE anything is clicked.
    let temp = std::env::temp_dir();
    let packages = ctx
        .ensure_world()
        .sandbox()
        .local
        .join("Flare")
        .join("packages");
    let dirs_before = names_with(&temp, "flare-update");
    let pkgs_before = names_with(&packages, "");
    let installers_before = processes_naming("flare-update-");

    // PT: run.mjs:7335-7336. A click opens the window and installs nothing.
    click_chip(app);
    let opened = ctx.until("the window to open", 4_000, &mut || is_up(app));
    ctx.ok(opened, "a click on the chip opens the window");
    ctx.until("the first calls file", 3_000, &mut || calls(app).is_some());
    ctx.ok(
        calls(app).map(|c| c.1) == Some(0),
        "and installs nothing: the click used to download and quit",
    );
    let w = uw(app);
    // PT: run.mjs:7337.
    ctx.ok(
        w["title"] == json!(format!("Update to {next}")),
        "its title names the version",
    );
    // PT: run.mjs:7338.
    ctx.ok(
        w["subtitle"] == json!(format!("You have {version}")),
        format!("and a quiet line says what is running ({version})"),
    );
    let entries = entries_of(&w);
    // PT: run.mjs:7340.
    ctx.ok(
        entries.len() >= 5,
        format!("the sample notes are listed ({} entries)", entries.len()),
    );
    // PT: run.mjs:7341.
    ctx.ok(
        entries
            .first()
            .is_some_and(|e| e.starts_with("The update button opens a window")),
        format!(
            "as the pull requests' titles (\"{}\")",
            entries.first().cloned().unwrap_or_default()
        ),
    );
    // PT: run.mjs:7345-7347. Sorted under headings, worded for a reader.
    let sections = sections_of(&w);
    let heads: Vec<String> = sections.iter().map(|s| s.0.clone()).collect();
    ctx.ok(
        heads.join("|") == "New features|Bug fixes|Under the hood",
        format!(
            "the notes are sorted under headings ({})",
            sections
                .iter()
                .map(|(h, l)| format!("{h}: {}", l.len()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    let fix = sections
        .get(1)
        .and_then(|s| s.1.first())
        .cloned()
        .unwrap_or_default();
    ctx.ok(
        fix == "A prompt survives the window getting narrower and wider again",
        format!("a fix reads as a sentence, its \"fix(terminal):\" gone (\"{fix}\")"),
    );
    // PT: run.mjs:7348.
    ctx.ok(
        entries.iter().all(|e| {
            !ends_with_pr_number(e)
                && !starts_with_commit_type(e)
                && e.chars().next().is_some_and(char::is_uppercase)
        }),
        "no line ends in a pull request number or starts with a commit type, and each starts with a capital",
    );
    let text = all_text(&w);
    // PT: run.mjs:7350-7352.
    ctx.ok(
        !text.contains("by @"),
        "no author tail (\"by @\") reaches the window",
    );
    ctx.ok(
        !text.contains("http:") && !text.contains("https:") && !text.contains("github.com"),
        "and no url does",
    );
    ctx.ok(
        ![
            "Full Changelog",
            "New Contributors",
            "first contribution",
            "What's Changed",
        ]
        .iter()
        .any(|b| text.contains(b)),
        "nor the boilerplate round the list",
    );
    // PT: run.mjs:7353 and 7354. There is no element tree to hold an anchor: the notes are strings,
    // and a string with no markup in it cannot bring one.
    ctx.ok(
        !text.contains("](") && !text.contains("<a"),
        "there is no link in it",
    );
    ctx.ok(
        !text.contains('<') && !text.contains('>'),
        "the notes are text and nothing a body could have brought with it",
    );
    // PT: run.mjs:7358.
    ctx.ok(
        !text.contains("This is a preview") && w["status"] == json!(""),
        "a preview is not announced up front: the window is shown as it will be",
    );

    // PT: run.mjs:7359-7362 and 7376-7390. The notes sit on the window's own ground, and the window
    // is on the opaque surface. Both are read off the pixels.
    let dialog_look = look(ctx, app, "dialog-dark");
    let ground = dialog_look.as_ref().and_then(|(img, k)| {
        let (x, y) = surface_at(&w, 0.5)?;
        Some(px(img, *k, x, y))
    });
    let in_notes = dialog_look.as_ref().and_then(|(img, k)| {
        let (bx, _, bw, _) = rc(&w["box"])?;
        let ny = w["notes"]["y"].as_f64()?;
        let nh = w["notes"]["h"].as_f64()?;
        Some(px(img, *k, bx + bw - 24.0, ny + nh / 2.0))
    });
    ctx.ok(
        matches!((ground, in_notes), (Some(g), Some(n)) if near(g, n, 1)),
        format!(
            "the notes sit on the window's own ground: no fill, no border ({in_notes:?} against {ground:?})"
        ),
    );
    let surface: Vec<Rgb> = dialog_look
        .as_ref()
        .map(|(img, k)| {
            [0.15, 0.5, 0.9]
                .iter()
                .filter_map(|f| surface_at(&w, *f).map(|(x, y)| px(img, *k, x, y)))
                .collect()
        })
        .unwrap_or_default();
    ctx.ok(
        surface.len() == 3 && surface.iter().all(|c| near(*c, surface[0], 1)),
        format!(
            "the window is on the opaque surface: the terminal does not show through ({surface:?})"
        ),
    );
    // PT: run.mjs:7391-7393. The list scrolls inside a capped height; the whole window is on screen.
    let st = probe(app, "app.state", Value::Null);
    let scale = st["scale"].as_f64().unwrap_or(1.0).max(0.1);
    let win_h = st["window"]["client_h"].as_f64().unwrap_or(0.0) / scale;
    let win_w = st["window"]["client_w"].as_f64().unwrap_or(0.0) / scale;
    let cap = (0.44 * win_h).min(300.0);
    let notes_h = w["notes"]["h"].as_f64().unwrap_or(0.0);
    ctx.ok(
        notes_h > 0.0 && notes_h <= cap + 0.5,
        format!("the list scrolls inside a capped height ({notes_h:.1} px of at most {cap:.1})"),
    );
    let (bx, by, bw, bh) = rc(&w["box"]).unwrap_or((-1.0, -1.0, 0.0, 0.0));
    ctx.ok(
        bx >= 0.0 && by >= 0.0 && bx + bw <= win_w + 0.5 && by + bh <= win_h + 0.5,
        "the whole window is on screen",
    );
    ctx.ok(
        buttons_of(&w) == "Cancel|Install",
        format!(
            "the choices are Cancel and Install, in that order ({})",
            buttons_of(&w)
        ),
    );
    ctx.ok(
        w["focus_idx"] == json!(1),
        "Install is the primary, and holds the focus",
    );

    // PT: run.mjs:7396-7410. Every way out.
    let _ = app.session.key("Escape");
    let esc = closed(ctx, app);
    ctx.ok(esc, "Escape closes it");
    ctx.ok(
        chip_shown(app) && chip_of(app)["label"] == json!(format!("Update {next}")),
        "and the offer is still there",
    );
    open_window(ctx, app);
    click_cancel(app);
    let cancelled = closed(ctx, app);
    ctx.ok(cancelled, "Cancel closes it");
    open_window(ctx, app);
    let _ = app.session.click(8.0, 300.0);
    let outside = closed(ctx, app);
    ctx.ok(outside, "and so does a press outside it");
    ctx.ok(
        calls(app).map(|c| c.1) == Some(0),
        "none of which installed anything",
    );

    // PT: run.mjs:7414-7415. Install: the window stays, and is sampled the whole way.
    open_window(ctx, app);
    let w = uw(app);
    let track_h = w["track"]["h"].as_f64().unwrap_or(0.0);
    ctx.ok(
        w["active"] == json!(false) && (track_h - 6.0).abs() < 0.5,
        format!(
            "before Install the progress track is already in the layout, unseen ({track_h:.1} px, active {})",
            w["active"]
        ),
    );
    let mut samples: Vec<Sample> = vec![sample(app)];
    click_install(app);
    let started = sampling(
        ctx,
        app,
        &mut samples,
        "the download to start",
        6_000,
        &|s| s.phase == "downloading",
    );
    ctx.ok(started, "Install starts the download");
    ctx.ok(
        is_up(app),
        "and THE WINDOW STAYS UP: the progress is straight there",
    );
    // Not dismissable while it runs.
    let _ = app.session.key("Escape");
    let _ = app.session.click(8.0, 300.0);
    ctx.settle(50);
    ctx.ok(
        is_up(app) && phase_of(app) != "idle",
        "Escape and a press outside do NOT put a running install away",
    );
    sampling(ctx, app, &mut samples, "35 percent", 6_000, &|s| {
        s.pct >= 35
    });
    let _ = ctx.shot(app, "progress-dark");
    let ended = sampling(
        ctx,
        app,
        &mut samples,
        "the fake install to end",
        10_000,
        &|s| s.phase == "idle" && s.status == PREVIEW_DONE,
    );
    ctx.ok(ended, "the fake install ends by saying so IN the window");
    let _ = ctx.shot(app, "ended-dark");
    let ended_w = uw(app);

    ctx.ok(
        samples.len() > 40 && samples.iter().all(|s| s.up),
        format!(
            "the window was up in every one of {} samples",
            samples.len()
        ),
    );
    let mut phases: Vec<&str> = samples.iter().map(|s| s.phase.as_str()).collect();
    phases.sort_unstable();
    phases.dedup();
    ctx.ok(
        ["idle", "downloading", "installing"]
            .iter()
            .all(|p| phases.contains(p)),
        format!("it went through every phase ({})", phases.join(", ")),
    );
    let running: Vec<&Sample> = samples.iter().filter(|s| s.phase != "idle").collect();
    let pcts: Vec<i64> = running.iter().map(|s| s.pct).collect();
    ctx.ok(
        pcts.len() > 5
            && pcts
                .iter()
                .enumerate()
                .all(|(i, v)| *v >= 0 && (i == 0 || *v >= pcts[i - 1]))
            && pcts.last() == Some(&100),
        format!(
            "the bar only rises, to 100 ({}% to {}%)",
            pcts.first().copied().unwrap_or(-1),
            pcts.last().copied().unwrap_or(-1)
        ),
    );
    ctx.ok(
        running.iter().all(|s| s.pct_text == format!("{}%", s.pct)),
        "and the number beside it is the same number",
    );
    ctx.ok(
        running
            .iter()
            .all(|s| s.title == format!("Updating to {next}")),
        "the title says what is happening while it runs",
    );
    ctx.ok(
        samples
            .iter()
            .any(|s| s.phase == "downloading" && s.status == "Downloading the update"),
        "the status line says it is downloading",
    );
    ctx.ok(
        samples
            .iter()
            .any(|s| s.phase == "installing" && s.status.starts_with("Installing")),
        "and then that it is installing",
    );
    ctx.ok(
        running.iter().all(|s| !s.install_on),
        "Install cannot be pressed twice",
    );
    ctx.ok(
        samples
            .iter()
            .filter(|s| s.phase == "downloading")
            .all(|s| s.cancel_label == "Cancel" && s.cancel_on),
        "Cancel can be pressed for as long as it is DOWNLOADING",
    );
    ctx.ok(
        samples
            .iter()
            .filter(|s| s.phase == "installing")
            .all(|s| !s.cancel_on),
        "and not once the installer has the file",
    );
    let mut boxes: Vec<&str> = samples.iter().map(|s| s.boxed.as_str()).collect();
    boxes.sort_unstable();
    boxes.dedup();
    ctx.ok(
        boxes.len() == 1,
        format!(
            "THE WINDOW NEVER CHANGED SIZE OR MOVED, from the notes to the bar to the ending ({})",
            boxes.join(", ")
        ),
    );
    let same =
        |f: &dyn Fn(&Sample) -> f64, want: f64| samples.iter().all(|s| (f(s) - want).abs() < 0.01);
    ctx.ok(
        same(&|s| s.chip_w, width0),
        format!("the chip's width is identical throughout (idle {width0:.3} px)"),
    );
    ctx.ok(same(&|s| s.chip_x, left0), "and its left edge never moved");
    ctx.ok(
        samples
            .iter()
            .all(|s| s.chip_label == format!("Update {next}")),
        "its label never changes: the bar is the window's, not the chip's",
    );
    ctx.ok(
        samples.iter().all(|s| s.notice.is_empty()),
        "and nothing is hung under the chip while the window is there to say it",
    );
    ctx.ok(
        buttons_of(&ended_w) == "Close|Install",
        format!(
            "afterwards the choices are Close and Install again ({})",
            buttons_of(&ended_w)
        ),
    );
    ctx.ok(
        ended_w["running"] == json!(false) && ended_w["pct"] == json!(0),
        "with the bar gone: it does not drain back to nothing",
    );
    let _ = app.session.key("Escape");
    let again = closed(ctx, app);
    ctx.ok(again, "it can be closed again now, with Escape");
    let quiet = never(ctx, app, 300, &mut |a| {
        !uw(a)["notice"].as_str().unwrap_or("").is_empty()
    });
    ctx.ok(
        quiet,
        "and having been read there, the line is not repeated under the chip",
    );
    ctx.ok(
        chip_of(app)["busy"] == json!(false)
            && chip_of(app)["label"] == json!(format!("Update {next}")),
        "the chip offers the same update",
    );

    // PT: run.mjs:7448-7466. CANCEL, mid-download. It is not a failure and is not reported as one.
    open_window(ctx, app);
    let mut second: Vec<Sample> = vec![sample(app)];
    click_install(app);
    sampling(ctx, app, &mut second, "20 percent", 6_000, &|s| s.pct >= 20);
    click_cancel(app);
    let gone = closed(ctx, app);
    ctx.ok(
        gone,
        "Cancel during the download stops it and the window goes",
    );
    never(ctx, app, 1_200, &mut |a| {
        second.push(sample(a));
        false
    });
    let top = second.iter().map(|s| s.pct).max().unwrap_or(0);
    ctx.ok(
        !second.iter().any(|s| s.phase == "installing") && top < 100,
        format!("it never reached the installer (stopped at {top}%)"),
    );
    ctx.ok(
        sample(app).notice.is_empty() && !is_up(app),
        "nothing is said about it: the user knows what they pressed",
    );
    ctx.ok(
        chip_of(app)["busy"] == json!(false),
        "and the chip offers the update again",
    );

    // PT: run.mjs:7468-7478. What a preview must not have done.
    let c = calls(app);
    ctx.ok(
        c.map(|c| c.0) == Some(0),
        format!(
            "GitHub was never asked ({:?} release checks)",
            c.map(|c| c.0)
        ),
    );
    ctx.ok(
        c.map(|c| c.1) == Some(0),
        format!("no download was started ({:?} installs)", c.map(|c| c.1)),
    );
    ctx.ok(
        names_with(&temp, "flare-update") == dirs_before
            && names_with(&packages, "") == pkgs_before,
        "no installer was downloaded: no update folder appeared in temp or in the packages folder",
    );
    let installers_after = processes_naming("flare-update-");
    ctx.ok(
        installers_before.is_some() && installers_after == installers_before,
        format!("no installer process was spawned ({installers_before:?} before, {installers_after:?} after)"),
    );
    ctx.ok(
        app.child.exited().is_none() && probe(app, "app.state", Value::Null)["version"].is_string(),
        "and the app never quit",
    );
    let front = app
        .session
        .state()
        .ok()
        .and_then(|s| s.front)
        .unwrap_or_default();
    let tabs = app.session.state().map(|s| s.tabs.len()).unwrap_or(0);
    ctx.ok(
        tabs == 1 && rows_text(app, &front).contains('>'),
        "with its shell still running",
    );

    // PT: run.mjs:7480-7525. The same, on a light theme.
    let light = light_theme(ctx, app);
    ctx.ok(light, "on a light theme");
    let c = chip_of(app);
    let width_light = c["rect"]["w"].as_f64().unwrap_or(-1.0);
    ctx.ok(
        (width_light - width0).abs() < 0.01,
        format!("the chip is the same width ({width_light:.3} px)"),
    );
    let _ = ctx.shot(app, "chip-light");
    let over = open_window(ctx, app);
    ctx.ok(over, "the window opens over Settings too");
    let w = uw(app);
    let lt = look(ctx, app, "dialog-light");
    let ground = lt.as_ref().and_then(|(img, k)| {
        let (x, y) = surface_at(&w, 0.5)?;
        Some(px(img, *k, x, y))
    });
    ctx.ok(
        ground.is_some_and(|g| lum(g) > 0.4),
        format!(
            "its surface is light (luminance {:.2})",
            ground.map(lum).unwrap_or(0.0)
        ),
    );
    let read = match (&lt, ground, rc(&w["box"])) {
        (Some((img, k)), Some(g), Some((bx, _, bw, _))) => {
            let ny = w["notes"]["y"].as_f64().unwrap_or(0.0);
            let nh = w["notes"]["h"].as_f64().unwrap_or(0.0);
            best_contrast(img, *k, (bx + 20.0, ny, bx + bw - 28.0, ny + nh), g)
        }
        _ => 0.0,
    };
    ctx.ok(
        read >= 4.5,
        format!("and the notes read on it ({read:.1}:1)"),
    );
    let tk = tokens(app);
    let sel_bg = rgb_of_hex(tk["sel_bg"].as_str().unwrap_or("#000000"));
    let on_accent = rgb_of_hex(tk["on_accent"].as_str().unwrap_or("#ffffff"));
    let ink_light = contrast(on_accent, sel_bg);
    ctx.ok(
        ink_light >= 4.5,
        format!("the chip's label reads on a light theme's accent too ({ink_light:.1}:1)"),
    );
    click_install(app);
    ctx.until("35 percent", 6_000, &mut || pct_of(app) >= 35);
    let pr = look(ctx, app, "progress-light");
    let w = uw(app);
    let share = {
        let (t, f) = (
            w["track"]["w"].as_f64().unwrap_or(1.0),
            w["fill"]["w"].as_f64().unwrap_or(0.0),
        );
        f / t
    };
    ctx.ok(
        share > 0.2 && share <= 1.0,
        format!(
            "on the light theme the bar is drawn, and filled to the percentage ({}%)",
            (share * 100.0).round()
        ),
    );
    let status_read = match (&pr, rc(&w["box"]), rc(&w["track"])) {
        (Some((img, k)), Some((bx, _, bw, _)), Some((_, ty, _, _))) => {
            let g = ground.unwrap_or([255, 255, 255]);
            best_contrast(img, *k, (bx + 20.0, ty - 24.0, bx + bw - 70.0, ty - 6.0), g)
        }
        _ => 0.0,
    };
    ctx.ok(
        status_read >= 4.5,
        format!("and the status line reads ({status_read:.1}:1)"),
    );
    let finished = ctx.until("the second preview to end", 10_000, &mut || {
        phase_of(app) == "idle" && status_of(app).contains("Preview only")
    });
    ctx.ok(finished, "a second preview install runs to its end as well");
    click_cancel(app);
    let closed_again = closed(ctx, app);
    ctx.ok(closed_again, "and Close closes it");
}

/// Opens Settings, goes to Appearance and presses the light preset's card; true once the window
/// paints a light theme.
fn light_theme(ctx: &mut Ctx, app: &mut App) -> bool {
    let _ = app.session.key("Ctrl+Comma");
    ctx.until("Settings", 8_000, &mut || {
        probe(app, "settings.state", Value::Null)["front"] == json!(true)
    });
    let rail = probe(app, "settings.rail", json!({ "id": "appearance" }));
    click_v(app, &rail);
    ctx.until("the Appearance page", 6_000, &mut || {
        probe(app, "settings.rows", json!({}))["page"] == json!("appearance")
    });
    let paper = pick_wall_card(ctx, app, "paper");
    paper
        && ctx.until("a light theme", 8_000, &mut || {
            tokens(app)["light"] == json!(true)
        })
}

/// Wheels the Settings pane until `find` says its box is inside the pane. `find` answers the box's
/// top edge and whether it is visible, or None while the thing is not drawn.
fn bring_into_view(
    ctx: &mut Ctx,
    app: &mut App,
    what: &str,
    find: &mut dyn FnMut(&mut App) -> Option<(f64, bool)>,
) -> bool {
    let grid =
        rc(&probe(app, "ui.rect", json!({ "name": "grid" }))).unwrap_or((0.0, 36.0, 1000.0, 600.0));
    let at = ((grid.0 + 500.0) as f32, (grid.1 + grid.3 / 2.0) as f32);
    let mid = grid.1 + grid.3 / 2.0;
    ctx.until(
        &format!("{what} to be in view"),
        12_000,
        &mut || match find(app) {
            Some((_, true)) => true,
            Some((y, false)) => {
                let dy = if y > mid { -160.0 } else { 160.0 };
                let _ = app.session.call(&Request::Pointer {
                    x: at.0,
                    y: at.1,
                    action: PointerAction::Wheel { dx: 0.0, dy },
                });
                false
            }
            None => false,
        },
    )
}

/// Opens the theme wall, brings a card into view and presses it; true when it is on afterwards.
fn pick_wall_card(ctx: &mut Ctx, app: &mut App, id: &str) -> bool {
    let card_of = |app: &mut App| -> Option<Value> {
        probe(app, "picker.wall", Value::Null)["cards"]
            .as_array()?
            .iter()
            .find(|c| c["id"] == json!(id))
            .cloned()
    };
    if probe(app, "picker.wall", Value::Null)["expanded"] == json!(false) {
        bring_into_view(ctx, app, "the wall's toggle", &mut |a| {
            let w = probe(a, "picker.wall", Value::Null);
            Some((
                w["toggle"]["y"].as_f64()?,
                w["toggle_visible"] == json!(true),
            ))
        });
        let toggle = probe(app, "picker.wall", Value::Null)["toggle"].clone();
        click_v(app, &toggle);
        ctx.until("the wall to open", 4_000, &mut || {
            probe(app, "picker.wall", Value::Null)["expanded"] == json!(true)
        });
    }
    if !bring_into_view(ctx, app, &format!("the {id} card"), &mut |a| {
        let c = card_of(a)?;
        Some((c["rect"]["y"].as_f64()?, c["visible"] == json!(true)))
    }) {
        return false;
    }
    let Some(card) = card_of(app) else {
        return false;
    };
    click_v(app, &card["rect"]);
    ctx.until(&format!("the {id} card to be on"), 4_000, &mut || {
        card_of(app).is_some_and(|c| c["on"] == json!(true))
    })
}
// ---- updateGuard ----------------------------------------------------------------------------------------------------

/// The steps of the agent stand-in: the idle title at once; once `go` exists the working title and
/// its hook line; once `idle` exists the idle title and a finished hook again.
fn steps_script(go: &Path, idle: &Path) -> String {
    // The hook lines `hook_line` writes, minus their ESC and BEL, which the script adds itself.
    let strip = |s: String| {
        s.trim_start_matches('\u{1b}')
            .trim_end_matches('\u{7}')
            .to_string()
    };
    let working = strip(hook_line("working", &json!({})));
    let done = strip(hook_line("done", &json!({})));
    format!(
        "$ErrorActionPreference = 'Stop'\r\n\
         $e = [string][char]27\r\n\
         $b = [string][char]7\r\n\
         [Console]::Write($e + ']0;{IDLE_TITLE}' + $b)\r\n\
         while (-not (Test-Path -LiteralPath '{go}')) {{ Start-Sleep -Milliseconds 50 }}\r\n\
         [Console]::Write($e + ']0;{WORKING_TITLE}' + $b)\r\n\
         [Console]::Write($e + '{working}' + $b)\r\n\
         while (-not (Test-Path -LiteralPath '{idle}')) {{ Start-Sleep -Milliseconds 50 }}\r\n\
         [Console]::Write($e + ']0;{IDLE_TITLE}' + $b)\r\n\
         [Console]::Write($e + '{done}' + $b)\r\n\
         Start-Sleep -Seconds 3600\r\n",
        go = go.display(),
        idle = idle.display(),
    )
}

/// Writes the stand-in's steps (with a byte-order mark, so Windows PowerShell reads the glyphs).
fn write_steps(claude: &mut FakeClaude, go: &Path, idle: &Path) -> Result<(), String> {
    claude.script(&[Step::Title(IDLE_TITLE.into())]);
    let body = steps_script(go, idle);
    let bytes = [b"\xEF\xBB\xBF".as_slice(), body.as_bytes()].concat();
    std::fs::write(claude.bin().join("claude-steps.ps1"), bytes)
        .map_err(|e| format!("write the stand-in's steps: {e}"))
}

/// The `app.state` answer.
fn app_state(app: &mut App) -> Value {
    probe(app, "app.state", Value::Null)
}

/// True while a question (the close question) is up.
fn dialog_open(app: &mut App) -> bool {
    app_state(app)["dialog"]["open"] == json!(true)
}

/// The question's title and body.
fn dialog_text(app: &mut App) -> String {
    let d = &app_state(app)["dialog"];
    format!(
        "{} {}",
        d["title"].as_str().unwrap_or(""),
        d["body"].as_str().unwrap_or("")
    )
}

/// The question's confirm button.
fn dialog_primary(app: &mut App) -> String {
    app_state(app)["dialog"]["buttons"][1]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// Which of the question's buttons the keys reach.
fn dialog_focus(app: &mut App) -> String {
    probe(app, "dialog.focus", Value::Null)["focused"]
        .as_str()
        .unwrap_or("none")
        .to_string()
}

/// The first tab's mark state: `working` while the agent works.
fn tab_mark(app: &mut App) -> String {
    app_state(app)["tabs"][0]["mark"]["state"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// True when the first tab hosts Claude (any state).
fn agent_present(app: &mut App) -> bool {
    app_state(app)["tabs"][0]["kind"] == json!("claude")
}

/// The number of tabs.
fn tab_count(app: &mut App) -> usize {
    app_state(app)["tabs"].as_array().map_or(0, Vec::len)
}

/// PT: run.mjs:7556-7653. A working agent is asked about before an install; a failed install says so
/// in the window; the notes of a release with none say so.
pub fn update_guard(ctx: &mut Ctx) -> Result<(), String> {
    let (alpha, mut claude) = {
        let w = ctx.ensure_world();
        (w.alpha.clone(), FakeClaude::new(w))
    };
    let go: PathBuf = claude.dir.join("go-working");
    let idle: PathBuf = claude.dir.join("go-idle");
    write_steps(&mut claude, &go, &idle)?;
    let mut env = claude.env();
    env.push((OFFER_VAR.into(), OFFER_URL.into()));
    let mut app = ctx.launch(Launch {
        folders: vec![alpha],
        renderer: Some("software"),
        env,
        ..Launch::default()
    })?;
    guard_checks(ctx, &mut app, &go, &idle);
    app.close();
    Ok(())
}

fn guard_checks(ctx: &mut Ctx, app: &mut App, go: &Path, idle: &Path) {
    ctx.until("the one tab", 15_000, &mut || tab_count(app) == 1);
    // PT: run.mjs:7573.
    let shown = ctx.until("the chip", 8_000, &mut || chip_shown(app));
    ctx.ok(shown, "a real-shaped offer shows the chip");

    // PT: run.mjs:7577-7581. WHAT MAIN OFFERED IS INSTALLED, NEVER A URL ANYTHING ELSE SENT. Flare has
    // no page and no bridge; the only outside input is the command file. A command that names a
    // real-looking past release is none of its three words, so nothing may be fetched or started.
    ctx.until("the calls file", 3_000, &mut || calls(app).is_some());
    let before = calls(app);
    command(
        app,
        "install https://github.com/Maxaubert/Flare/releases/download/v0.1.0/Flare-Setup-0.1.0.exe",
    );
    let read = ctx.until("the app to read the command", 5_000, &mut || {
        !command_pending(app)
    });
    ctx.settle(50);
    ctx.ok(
        read && before.is_some() && calls(app) == before && phase_of(app) == "idle",
        "an install for any url but the offered one is refused, and nothing is fetched",
    );

    // As closeAsk does: Claude stands in through a program of that name, idle first, working later.
    let front = app
        .session
        .state()
        .ok()
        .and_then(|s| s.front)
        .unwrap_or_default();
    wait_for_prompt(
        app,
        &front,
        Shell::PowerShell,
        prompt_timeout(Shell::PowerShell),
    );
    let _ = app.session.type_text("claude");
    let _ = app.session.key("Enter");
    ctx.until("an agent in the tab", 8_000, &mut || agent_present(app));

    // PT: run.mjs:7592-7599. The window of a release with no notes.
    let opened = open_window(ctx, app);
    ctx.ok(opened, "the chip opens the window");
    let w = uw(app);
    ctx.ok(
        entries_of(&w).is_empty(),
        "a release with no notes lists nothing",
    );
    ctx.ok(
        w["empty_note"]
            .as_str()
            .unwrap_or("")
            .contains("No notes were published"),
        "and says so in one line",
    );
    let tip = w["chip"]["tip"].as_str().unwrap_or("").to_string();
    ctx.ok(
        !tip.starts_with("Preview") && !all_text(&w).to_lowercase().contains("preview"),
        "a real offer is not called a preview",
    );
    let _ = ctx.shot(app, "dialog-empty");

    // PT: run.mjs:7600-7611. ONE QUESTION AT A TIME. The app's chords still work over the update
    // window, and this tab hosts an agent, so Ctrl+W asks; the window gives way to the question.
    let _ = app.session.key("Ctrl+W");
    let asked = ctx.until("the question", 4_000, &mut || dialog_open(app));
    ctx.ok(
        asked,
        "Ctrl+W over the update window still asks about the agent",
    );
    let gave_way = ctx.until("the update window to give way", 4_000, &mut || !is_up(app));
    ctx.ok(
        gave_way,
        "and the update window gives way, so the question is not hidden under it",
    );
    ctx.ok(
        dialog_primary(app) == "Close tab" && dialog_focus(app) == "primary",
        "the focus is on the question that can be seen",
    );
    let _ = app.session.key("Escape");
    let backed = ctx.until("the question to close", 4_000, &mut || !dialog_open(app));
    ctx.ok(backed, "Escape backs out of it");
    ctx.ok(
        tab_count(app) == 1 && calls(app).map(|c| c.1) == Some(0),
        "with the tab still open and nothing installed",
    );

    // The window is put away here whatever it did above, so each check that follows stands alone
    // (a window that stayed up already failed the check about giving way).
    if is_up(app) {
        click_cancel(app);
        closed(ctx, app);
    }

    // PT: run.mjs:7613-7623. A working agent: Install asks first.
    let _ = std::fs::write(go, b"");
    let working = ctx.until("the agent to work", 8_000, &mut || {
        tab_mark(app) == "working"
    });
    ctx.ok(working, "an agent is mid-answer");
    open_window(ctx, app);
    click_install(app);
    let q = ctx.until("the install question", 4_000, &mut || dialog_open(app));
    ctx.ok(q, "Install asks before it does anything");
    ctx.ok(
        !is_up(app),
        "from in front of the update window, not from behind it",
    );
    let asked = dialog_text(app);
    let primary = dialog_primary(app);
    ctx.ok(
        asked.contains("Stop the agent and install the update?"),
        format!(
            "in its own words (\"{}\")",
            asked.chars().take(40).collect::<String>()
        ),
    );
    ctx.ok(
        asked.contains("Claude")
            && asked.contains("working for")
            && primary == "Install and restart",
        "naming the agent, how long it has worked, and what yes means",
    );
    let _ = ctx.shot(app, "agent-question");
    let _ = app.session.key("Escape");
    let backed = ctx.until("the question to close", 4_000, &mut || !dialog_open(app));
    ctx.ok(backed, "Cancel backs out");
    ctx.ok(
        calls(app).map(|c| c.1) == Some(0) && chip_of(app)["busy"] == json!(false),
        "and nothing was started",
    );

    // PT: run.mjs:7633-7650. The go-ahead; the failure says so in the window. (A Cancel on the install
    // question brings the window back, so it may already be up.)
    if !is_up(app) {
        open_window(ctx, app);
    }
    click_install(app);
    ctx.until("the install question", 4_000, &mut || dialog_open(app));
    // Enter presses the primary, which holds the keys (the probe `dialog.focus` says so).
    let _ = app.session.key("Enter");
    let started = ctx.until("the install to start", 6_000, &mut || {
        calls(app).map(|c| c.1) == Some(1)
    });
    ctx.ok(started, "the go-ahead starts the install");
    let said = ctx.until("the failure said in the window", 8_000, &mut || {
        is_up(app) && status_of(app).contains("could not be downloaded")
    });
    ctx.ok(
        said,
        "an install that fails says so IN the update window, which came back after the question",
    );
    ctx.ok(
        uw(app)["notice"] == json!(""),
        "and not a second time under the chip",
    );
    let offered = ctx.until("the chip to offer again", 4_000, &mut || {
        chip_of(app)["busy"] == json!(false)
    });
    ctx.ok(offered, "the chip offers the update again");
    let _ = ctx.shot(app, "failed");
    ctx.ok(
        uw(app)["buttons"][0]["label"] == json!("Close"),
        "the way out is called Close",
    );
    click_cancel(app);
    let gone = closed(ctx, app);
    ctx.ok(gone, "and closes it");

    // PT: run.mjs:7651-7655. A failed install must not leave the close question pre-answered: the
    // agent is still working, so closing the window still asks.
    let close = probe(app, "ui.rect", json!({ "name": "close" }));
    click_v(app, &close);
    let again = ctx.until("the window question", 4_000, &mut || dialog_open(app));
    ctx.ok(
        again,
        "closing the window still asks about the agent afterwards",
    );
    ctx.ok(
        dialog_text(app).contains("close the window"),
        "with the window question, not the install one",
    );
    let _ = app.session.key("Escape");
    // End idle, or the close below is held on the question.
    let _ = std::fs::write(idle, b"");
    ctx.until("the agent to be idle", 8_000, &mut || {
        tab_mark(app) != "working"
    });
}

// ---- updateNotes ------------------------------------------------------------------------------------------------

/// The hostile, over-long release body of PT's `updateNotes`.
fn hostile_body() -> String {
    let mut lines: Vec<String> = vec![
        "## What's Changed".into(),
        "* <img src=x onerror=\"window.__pwned = 1\"> an image tag by @a in https://github.com/o/r/pull/1".into(),
        "* [a link](https://evil.example/login) and <a href=\"https://evil.example\">an anchor</a> by @a in https://github.com/o/r/pull/2".into(),
        "* <script>window.__pwned = 2</script> a script by @a in https://github.com/o/r/pull/3".into(),
        "* <iframe src=\"https://evil.example\"></iframe> a frame by @a in https://github.com/o/r/pull/4".into(),
    ];
    for i in 0..26 {
        lines.push(format!(
            "* Change number {} with a title long enough to wrap onto a second line in the window by @a in https://github.com/o/r/pull/{}",
            i + 5,
            i + 5
        ));
    }
    lines.push(String::new());
    lines.push("**Full Changelog**: https://github.com/o/r/compare/v1...v2".into());
    lines.join("\n")
}

/// PT: run.mjs:7654-7721. A hostile body becomes no element and runs no handler; the words survive,
/// the list is capped with a count, it scrolls inside, and the buttons stay on screen.
pub fn update_notes(ctx: &mut Ctx) -> Result<(), String> {
    let alpha = ctx.ensure_world().alpha.clone();
    let mut app = ctx.launch(Launch {
        folders: vec![alpha],
        renderer: Some("software"),
        env: vec![
            (OFFER_VAR.into(), OFFER_URL.into()),
            (NOTES_VAR.into(), hostile_body()),
        ],
        ..Launch::default()
    })?;
    notes_checks(ctx, &mut app);
    app.close();
    Ok(())
}

fn notes_checks(ctx: &mut Ctx, app: &mut App) {
    ctx.until("the one tab", 15_000, &mut || tab_count(app) == 1);
    ctx.until("the chip", 8_000, &mut || chip_shown(app));
    // PT: run.mjs:7692.
    let up = open_window(ctx, app);
    ctx.ok(up, "the window opens on a hostile body");
    let w = uw(app);
    let text = all_text(&w);
    // PT: run.mjs:7693-7694. No anchor, image, script or frame: the strings hold no markup, and the
    // handler that would have run opened nothing (there is no script to run; a link would open a url).
    let markup = text.contains('<') || text.contains('>') || text.contains("](");
    ctx.ok(
        !markup,
        "nothing the body asked for is an element (the notes hold no markup)",
    );
    let opened = probe(app, "url.log", Value::Null);
    let launched = probe(app, "open.log", Value::Null);
    ctx.ok(
        opened["urls"].as_array().is_none_or(Vec::is_empty)
            && launched["calls"].as_array().is_none_or(Vec::is_empty),
        "and its handler never ran: nothing was opened",
    );
    // PT: run.mjs:7695.
    ctx.ok(
        !text.contains("evil.example")
            && !text.contains("http:")
            && !text.contains("https:")
            && !text.contains("by @"),
        "no url or author is printed either",
    );
    // PT: run.mjs:7696.
    ctx.ok(
        text.contains("An image tag") && text.contains("A link and an anchor"),
        "what is left of each line is its words",
    );
    // PT: run.mjs:7697.
    let n = entries_of(&w).len();
    let more = w["more"].as_str().unwrap_or("").to_string();
    ctx.ok(
        n == 20 && more == "+ 10 more",
        format!("the list is capped and counts the rest ({n} shown, \"{more}\")"),
    );
    // PT: run.mjs:7698.
    let content = w["notes"]["content_h"].as_f64().unwrap_or(0.0);
    let shown = w["notes"]["h"].as_f64().unwrap_or(0.0);
    ctx.ok(
        content > shown + 1.0,
        format!("the list scrolls inside the window ({content:.0} px in {shown:.0})"),
    );
    // PT: run.mjs:7699.
    let st = app_state(app);
    let scale = st["scale"].as_f64().unwrap_or(1.0).max(0.1);
    let win_h = st["window"]["client_h"].as_f64().unwrap_or(0.0) / scale;
    let fits = |w: &Value, win_h: f64| {
        let (_, by, _, bh) = rc(&w["box"]).unwrap_or((0.0, -1.0, 0.0, 0.0));
        let (_, iy, _, ih) = rc(&w["install"]).unwrap_or((0.0, -1.0, 0.0, 0.0));
        let (_, cy, _, ch) = rc(&w["cancel"]).unwrap_or((0.0, -1.0, 0.0, 0.0));
        by >= 0.0
            && by + bh <= win_h + 0.5
            && iy >= 0.0
            && iy + ih <= win_h
            && cy >= 0.0
            && cy + ch <= win_h
    };
    ctx.ok(
        fits(&w, win_h),
        "and Cancel and Install stay on screen under it",
    );
    let _ = ctx.shot(app, "dialog-long");

    // PT: run.mjs:7700-7716. A small window: the list gives way, the buttons do not.
    let _ = app
        .session
        .call(&flare_e2e_proto::Request::Resize { w: 560.0, h: 400.0 });
    let small = ctx.until(
        "the whole dialog to fit the small window",
        6_000,
        &mut || {
            let st = app_state(app);
            let k = st["scale"].as_f64().unwrap_or(1.0).max(0.1);
            let h = st["window"]["client_h"].as_f64().unwrap_or(0.0) / k;
            let w = uw(app);
            h <= 420.0 && fits(&w, h)
        },
    );
    ctx.ok(
        small,
        "at the smallest window the whole dialog still fits, buttons included",
    );
    let _ = ctx.shot(app, "dialog-small");
    let _ = app.session.key("Escape");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_minor_follows_the_running_version() {
        assert_eq!(next_minor("0.1.0"), "0.2.0");
        assert_eq!(next_minor("v1.9.3"), "1.10.0");
        assert_eq!(next_minor("junk"), "0.1.0");
    }

    #[test]
    fn a_pull_request_number_is_only_a_hash_and_digits_in_brackets() {
        assert!(ends_with_pr_number("Links follow the theme (#33)"));
        assert!(ends_with_pr_number("Links follow the theme (#33)  "));
        assert!(!ends_with_pr_number("Links follow the theme (soon)"));
        assert!(!ends_with_pr_number("Links follow the theme (#)"));
        assert!(!ends_with_pr_number("Links follow the theme"));
    }

    #[test]
    fn a_commit_type_is_lower_case_letters_a_scope_and_a_colon() {
        assert!(starts_with_commit_type("fix(terminal): a prompt"));
        assert!(starts_with_commit_type("feat!: a break"));
        assert!(starts_with_commit_type("chore: tidy"));
        assert!(!starts_with_commit_type("A prompt survives"));
        assert!(!starts_with_commit_type("Dictation: speak"));
        assert!(!starts_with_commit_type("explorer entries: read"));
    }

    #[test]
    fn the_stand_in_waits_for_two_files_in_order() {
        let s = steps_script(Path::new(r"C:\t\go"), Path::new(r"C:\t\idle"));
        let work = s.find(WORKING_TITLE).expect("the working title");
        let idle_wait = s.find(r"C:\t\idle").expect("the idle file");
        let idle_title = s.rfind(IDLE_TITLE).expect("the idle title again");
        assert!(work < idle_wait && idle_wait < idle_title);
        assert!(
            !s.contains('\u{1b}'),
            "the ESC and BEL are added by the script"
        );
    }

    #[test]
    fn the_hostile_body_has_thirty_entries_and_every_trap() {
        let b = hostile_body();
        assert_eq!(b.matches("\n* ").count(), 30);
        for trap in [
            "<img",
            "<script",
            "<iframe",
            "evil.example",
            "Full Changelog",
        ] {
            assert!(b.contains(trap), "{trap}");
        }
    }

    #[test]
    fn colours_parse_from_hex_with_or_without_alpha() {
        assert_eq!(rgb_of_hex("#102030"), [0x10, 0x20, 0x30]);
        assert_eq!(rgb_of_hex("#10203080"), [0x10, 0x20, 0x30]);
        assert!(near([10, 10, 10], [11, 9, 10], 1));
        assert!(!near([10, 10, 10], [13, 10, 10], 2));
    }

    #[test]
    fn a_sample_set_with_two_boxes_is_two_boxes() {
        let a = Sample {
            up: true,
            boxed: "460x300@10,20".into(),
            phase: "idle".into(),
            title: String::new(),
            pct: 0,
            pct_text: "0%".into(),
            status: String::new(),
            cancel_label: "Cancel".into(),
            cancel_on: true,
            install_on: true,
            chip_w: 100.0,
            chip_x: 5.0,
            chip_label: String::new(),
            notice: String::new(),
        };
        let mut b = a.clone();
        b.boxed = "460x301@10,20".into();
        let mut all = vec![a.boxed.as_str(), b.boxed.as_str()];
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 2);
    }
}
